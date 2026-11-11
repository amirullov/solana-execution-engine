# Solana Execution Engine — Software Design Document

| Поле                | Значение                                                |
| ------------------- | ------------------------------------------------------- |
| Версия              | 1.0                                                     |
| Дата                | 11 ноября 2026                                          |
| Статус              | Approved for Stage 1 Foundation                         |
| Владелец            | Portfolio Web3                                          |
| Репозиторий         | `solana-execution-engine` (отдельно от Solana Sentinel) |
| Денежное исполнение | Запрещено на Stage 1                                    |

## 1. Резюме решения

Solana Execution Engine — сервисная платформа, которая принимает идемпотентное swap-намерение, получает и проверяет котировки, формирует контролируемый план транзакции, симулирует его, применяет risk policy, подписывает через изолированный signer и отслеживает outcome до финальности.

Архитектура делится на:

- TypeScript control plane: публичный API, аутентификация, оркестрация, административные операции и интеграции с REST API;
- Rust data plane: потоковые данные, каноническое состояние, policy engine, декодирование/симуляция и будущий broadcast/confirmation loop;
- PostgreSQL как источник истины и append-only audit trail;
- Redis только как cache, rate-limit и краткоживущая координация;
- Solana RPC + Yellowstone gRPC как взаимодополняющие источники, а не взаимозаменяемые единственные точки истины;
- Jupiter Swap API v2 как первый route/quote provider.

Stage 1 реализует безопасный Foundation. В нём нет приватных ключей, signer adapter и сетевого вызова отправки транзакций. Единственное допустимое значение `EXECUTION_MODE` — `disabled`.

## 2. Product specification

### 2.1 Проблема

Прямое исполнение Solana swap кажется короткой последовательностью «quote → sign → send», но production-система должна выдерживать устаревшие blockhash/quote, forks, частично доставленные gRPC banks, RPC rate limits, дубли, повторные запросы клиента, изменения API провайдера, congested landing и компрометацию control plane. Без независимой проверки инструкций и детерминированного журнала невозможно доказать, почему и с какими лимитами была подписана транзакция.

### 2.2 Пользователи

1. Стратегия или portfolio service создаёт swap intent с верхними лимитами риска.
2. Оператор наблюдает очередь, состояние провайдеров и причины отказов.
3. Security/reliability engineer расследует каждое решение по неизменяемому audit trail.
4. Разработчик подключает нового quote, RPC или broadcast provider через стабильный adapter contract.

### 2.3 Цели продукта

- Exactly-once business effect поверх at-least-once доставки.
- Ни одна подпись не создаётся без прошедшего policy snapshot и симуляции.
- Доказуемая связь intent → quote → instructions → simulation → policy → signature → outcome.
- Автоматическое безопасное восстановление после process restart, stream gap и RPC failover.
- Возможность начинать с devnet/paper mode и отдельно разрешать real-money окружение.

### 2.4 Не-цели первой версии

- HFT/MEV searcher и latency race на уровне shred/deshred.
- Custodial wallet product или управление пользовательскими депозитами.
- Собственный Solana validator, AMM или on-chain execution program.
- Cross-chain bridge execution.
- Гарантия цены: engine обеспечивает лимиты и наблюдаемость, но не устраняет market/landing risk.

### 2.5 Основные сценарии

**UC-1 Quote-only.** Клиент передаёт пару и amount; система возвращает нормализованный quote с временем, context slot, fee/slippage данными и provider evidence hash.

**UC-2 Simulation.** Engine получает инструкции, разрешает ALTs, декодирует программы и аккаунты, проверяет allowlist, симулирует и возвращает прогноз балансов/fees/CU без подписи и отправки.

**UC-3 Controlled execution (не ранее Stage 4).** После всех gates signer подписывает ровно hash одобренного message, broadcaster отправляет его по нескольким независимым каналам, confirmer отслеживает processed/confirmed/finalized и возможную замену fork.

**UC-4 Recovery.** После сбоя worker возобновляет intent по durable state. Новый signature не создаётся, пока не доказано, что прежний blockhash истёк и signature отсутствует во всех согласованных источниках.

**UC-5 Kill switch.** Оператор или автоматический SLO/risk trigger атомарно запрещает новые подписи; наблюдение и reconciliation продолжаются.

## 3. Требования

### 3.1 Функциональные

| ID   | Требование                                                                             |
| ---- | -------------------------------------------------------------------------------------- |
| F-01 | Принимать intents с idempotency key и целочисленным atomic amount.                     |
| F-02 | Получать quote минимум от Jupiter и хранить исходный ответ с SHA-256.                  |
| F-03 | Проверять mint/program/account allowlists, slippage, notional, fee и freshness.        |
| F-04 | Разбирать v0 transactions и Address Lookup Tables.                                     |
| F-05 | Симулировать на том же commitment/context, для которого выполнена pre-state проверка.  |
| F-06 | Подписывать только неизменённый canonical message hash через внешний signer.           |
| F-07 | Broadcast должен быть идемпотентным по signature и поддерживать независимые endpoints. |
| F-08 | Отслеживать state до finalized, включая dropped/dead/forked slots.                     |
| F-09 | Восстанавливать gap через RPC backfill и сверять результат с live stream.              |
| F-10 | Хранить append-only audit events и воспроизводимые policy/config versions.             |
| F-11 | Предоставлять health, readiness, metrics и trace correlation.                          |
| F-12 | Поддерживать глобальный, wallet, strategy, mint и provider kill switches.              |

### 3.2 Нефункциональные

| ID                | Цель production после Stage 5                                                                                       |
| ----------------- | ------------------------------------------------------------------------------------------------------------------- |
| N-01 Availability | 99.9% API и 99.95% confirmer в месяц, кроме объявленных окон.                                                       |
| N-02 Durability   | RPO 0 для принятых intents/audit events после ответа 202; PostgreSQL synchronous commit.                            |
| N-03 Recovery     | RTO ≤ 15 минут при потере одного сервиса; ≤ 60 минут при восстановлении БД из backup.                               |
| N-04 Latency      | p95 intent validation ≤ 75 ms без внешних вызовов; quote orchestration p95 ≤ 750 ms; policy evaluation p99 ≤ 20 ms. |
| N-05 Freshness    | p99 lag live finalized projection ≤ 3 slots в нормальном режиме.                                                    |
| N-06 Integrity    | 100% подписей имеют complete approval chain и matching message hash.                                                |
| N-07 Audit        | 100% terminal intents воспроизводятся из журнала и versioned config.                                                |
| N-08 Capacity     | 100 intents/s sustained и 300 intents/s burst 60 секунд без потери; масштабирование горизонтальное.                 |

Цели являются acceptance thresholds, а не заявлениями о текущей производительности. Они должны быть подтверждены измерениями перед production.

## 4. Контекст и границы доверия

```text
Strategy/User
    |
    v
[API Gateway/Auth] ---- [Control API / TypeScript]
                               |
                     durable intent + outbox
                               v
                         [PostgreSQL]
                               ^
                               |
 [Jupiter v2] <--> [Quote Adapter] --> [Rust Execution Engine] --> [Signer/HSM]
 [RPC A/B/C]  <--> [RPC Adapter]   --> [Policy + Simulation]     --> [Broadcasters]
 [Yellowstone] --> [Market Data]   --> [Canonical Slot Ledger]  --> [Confirmer]
                               |
                            [Redis]
                      non-authoritative only
```

Внешние провайдеры, входные данные клиента, Redis и control plane считаются потенциально скомпрометированными. Signer доверяет только короткоживущему mutually-authenticated запросу, содержащему hash, policy approval и environment-bound key identity.

## 5. Компоненты

### 5.1 TypeScript control plane

- `control-api`: authn/authz, schema validation, idempotency, status/read models;
- `quote-orchestrator` (Stage 2): параллельные quote adapters, timeout/circuit breaker, нормализация;
- `operator-api` (Stage 3): kill switches, policy rollout, replay/reconciliation requests;
- `packages/contracts`: versioned HTTP/event schemas; суммы передаются decimal strings.

TypeScript не подписывает транзакции и не является authoritative policy engine.

### 5.2 Rust data plane

- `engine-core`: state machine, policy decision, canonical hashes и domain invariants;
- `market-data`: Yellowstone/RPC ingestion, slot ledger, gap detector;
- `execution-engine`: instruction validation, ALT resolution, simulation, future signing workflow;
- `confirmer` (Stage 3): signature/slot outcome и fork compensation;
- `backfill-worker` (Stage 2): bounded RPC ranges, checkpoint convergence.

### 5.3 Хранилища

PostgreSQL хранит intents, immutable revisions, quotes, instruction bundles, simulations, policy decisions, signatures, attempts, slot facts, checkpoints и outbox. Изменение business state и outbox event выполняется в одной транзакции.

Redis хранит rate-limit counters, short-lived locks и cache. Потеря Redis не может потерять intent или изменить окончательный outcome.

Object storage в production хранит крупные compressed raw payloads, trace exports и отчёты replay; PostgreSQL сохраняет content hash и location.

## 6. Domain model и state machine

```text
RECEIVED
  -> VALIDATED
  -> QUOTED
  -> BUILT
  -> SIMULATED
  -> POLICY_APPROVED
  -> SIGNED
  -> BROADCAST
  -> PROCESSED
  -> CONFIRMED
  -> FINALIZED

Из любого pre-sign state: REJECTED | EXPIRED | CANCELLED
После SIGNED: FAILED_TO_LAND | ONCHAIN_ERROR | FINALIZED
При fork: PROCESSED/CONFIRMED -> ORPHANED -> RECONCILING
```

Переход задаётся командой с `expected_version`; update использует optimistic concurrency. Повтор той же команды возвращает предыдущий результат. Переход назад запрещён, кроме явной компенсации `ORPHANED/RECONCILING`, которая не удаляет историю.

Ключи идемпотентности:

- API: `(tenant_id, idempotency_key)`;
- quote: `(intent_id, provider, request_fingerprint, quote_epoch)`;
- simulation: `(message_hash, rpc_endpoint_generation, simulation_context_slot)`;
- signing: `(key_id, message_hash, policy_approval_id)`;
- broadcast: transaction signature;
- chain fact: `(slot, blockhash, fact_type, entity_id)`.

## 7. Потоки данных

### 7.1 Quote-only (Stage 2)

1. API нормализует и сохраняет intent, затем atomically создаёт outbox event.
2. Quote worker читает event at-least-once и делает Jupiter `/swap/v2/order` без `taker`.
3. Ответ проверяется schema, mint/amount, integer ranges, context/freshness и provider rate-limit metadata.
4. Raw response canonicalized, хэшируется и сохраняется; нормализованная проекция доступна клиенту.
5. Любой timeout/429/5xx даёт классифицированную ошибку и bounded retry с full jitter; intent не исполняется.

### 7.2 Future controlled execution

1. Worker запрашивает Jupiter Router `/build`, а не готовую непрозрачную отправку.
2. Rust независимо декодирует все instructions, signer/writable accounts, ALTs, Token/Token-2022 extensions и compute budget.
3. Проверяются program allowlist с pinned upgrade authority/code evidence, mint policy, fee caps, min-out и отсутствие неожиданных signers.
4. RPC A выполняет `simulateTransaction` с `sigVerify=false`, `replaceRecentBlockhash=true` для анализа; перед подписью строится окончательный message с новым blockhash и повторной симуляцией согласно policy.
5. Policy snapshot подписывает approval hash, включающий intent, quote, message, simulation, balances, slot, config version и expiry.
6. Signer проверяет environment/key/policy binding и подписывает только этот message hash.
7. Broadcaster отправляет те же bytes; повторная отправка использует ту же signature. Нельзя молча пересобрать транзакцию.
8. Confirmer сверяет минимум два источника и обновляет commitment. Бизнес-результат окончателен только на `finalized`.

### 7.3 Recovery

При restart выбираются non-terminal intents с lease timeout. Для `SIGNED/BROADCAST` сначала ищется существующая signature. Пока blockhash valid или outcome неизвестен, повторное подписание запрещено. После доказанного expiry создаётся новая execution revision с новым message hash и полным повтором quote/simulation/policy.

## 8. On-chain и off-chain design

### 8.1 On-chain

MVP не разворачивает собственную программу. Транзакция взаимодействует только с явно разрешёнными программами Jupiter route, System, Compute Budget, SPL Token/Token-2022 и выбранными AMM. Exact program IDs и executable account hashes/upgrade authorities версионируются в policy evidence.

Собственная on-chain escrow/vault программа рассматривается только если появятся атомарные multi-action workflows, delegated custody или protocol-level settlement. Это потребует отдельного SDD, аудита, formal invariants и governance plan.

### 8.2 Off-chain

Intents, quotes и policy решения находятся off-chain. Off-chain компоненты не доверяют serialized transaction провайдера: они декодируют и проверяют её содержимое. Decimal UI values никогда не попадают в критический путь; используются только atomic integer amounts (`u64`/decimal string/`NUMERIC(39,0)`).

### 8.3 Key management (future)

- ключи не хранятся в БД, `.env`, container image или application memory дольше операции;
- signer находится в отдельном network segment и использует HSM/KMS либо policy-enforcing remote signer;
- production key не доступен staging;
- dual control для создания/rotation/limit increase;
- per-key daily notional, per-transaction cap и destination/program constraints;
- аварийное disable не требует доступа к signer key.

## 9. Solana RPC integration

RPC pool содержит минимум двух коммерчески/операционно независимых провайдеров и опциональный собственный endpoint. Для каждого измеряются latency, slot skew, error/429 rate, first available block и feature support.

Используем:

- `getLatestBlockhash` + `lastValidBlockHeight`, без wall-clock предположений о TTL;
- `simulateTransaction` для preflight evidence;
- `getSignatureStatuses`/`getTransaction` для confirmation и meta;
- `getBlock`, `getBlocks`/`getBlocksWithLimit` для backfill;
- `getMultipleAccounts` для mint, ALT и balance pre-state;
- `getSlot`/`getBlockHeight`/`isBlockhashValid` для health и expiry.

Public endpoints не используются для production: официальная документация предупреждает о shared limits и отсутствии production guarantee. Запросы имеют deadline, retry budget, endpoint health score и hedging только для безопасных read methods. `sendTransaction` не ретраится с новыми bytes автоматически.

Commitment policy:

- `processed`: latency signal, никогда не окончательный accounting;
- `confirmed`: пользовательский progress и early anomaly detection;
- `finalized`: canonical checkpoint и terminal success.

## 10. Yellowstone gRPC integration

Yellowstone — основной low-latency stream для accounts, transactions, slots и block metadata. Клиент pin-ится к протестированной major version; proto/API upgrade проходит compatibility suite.

Проект учитывает актуальную reconnect model Yellowstone:

- replay запрашивается `from_slot` в первом содержательном subscribe request;
- reconnect stream может вернуть `Disconnected { partial_banks, replay_from_slot }`;
- все partial banks отбрасываются целиком до повторной доставки;
- replay может повторить ранее виденные updates, поэтому consumer всегда идемпотентен;
- checkpoint двигается только после получения полного bank boundary/block meta, записи всех facts и commit БД;
- отдельный gap detector сравнивает parent chain и finalized slot с RPC.

Фильтры минимальны: relevant transactions/accounts + slots/block meta. Большие payload limits задаются явно. Keepalive/ping, reconnect exponential backoff, token rotation и per-message size limits обязательны. Поток от одного провайдера не определяет финальность самостоятельно.

## 11. Reorg, fork, gap и backfill

### 11.1 Канонический slot ledger

Каждая наблюдаемая версия slot хранится как `(slot, blockhash, parent_slot, status, source, observed_at)`. Один номер slot может иметь несколько blockhash/fork observations. Проекции ссылаются на конкретные `(slot, blockhash)`, а не только slot.

### 11.2 Fork handling

1. `dead` status, несовпадающий parent или новый blockhash открывает fork incident.
2. Записи затронутой ветки помечаются `orphaned`; они не удаляются.
3. Projections откатываются компенсирующими событиями до общего предка.
4. Новая ветка применяется в parent order.
5. Если signature была только в orphaned branch, intent переходит в `RECONCILING`, а не автоматически пересылается.
6. После finalized соответствующая ветка становится canonical; conflicting provider запускает quarantine.

### 11.3 Gap detection

Gap возникает при reconnect, несовпадающем parent, slot lag выше порога, неизвестном диапазоне или checkpoint старше provider retention. Диапазон записывается durable job со start/end, причиной и source evidence.

### 11.4 Backfill algorithm

1. Зафиксировать watermark `live_finalized_at_start`.
2. От последнего finalized checkpoint запросить доступные slots bounded batches.
3. Получить blocks с ограниченным параллелизмом и retry budget.
4. Проверить parent linkage и blockhash; сохранить idempotently.
5. Если исторические данные pruned, переключиться на archive provider; отсутствие данных — hard degraded state, не silent skip.
6. Догнать watermark, затем обработать buffered live updates.
7. В одной транзакции применить facts/projection и сдвинуть checkpoint.
8. Сравнить два источника для всех execution-related signatures.

Backfill никогда не блокирует confirmer для уже известных signatures, но readiness ingestion становится degraded, если gap превышает SLO.

## 12. Jupiter integration

По состоянию на 11 ноября 2026 официальный путь — Swap API v2:

- Meta-Aggregator: `GET /swap/v2/order` + `POST /swap/v2/execute`;
- Router: `GET /swap/v2/build` + собственный RPC или Jupiter submission.

Foundation использует только `/order` без `taker`, поэтому API возвращает quote без транзакции. Для будущего engine выбран `/build`: raw instructions позволяют независимо проверить программы, accounts, min-out, compute budget, ALT и fees. Managed `/execute` не включается без отдельной оценки RFQ/last-look и provider trust.

Adapter обязан:

- передавать API key только через secret manager и никогда не логировать его;
- иметь schema/version guard и fixture-based compatibility tests;
- проверять echoed mints/amount, router, fee, price impact, route plan и response age;
- ограничивать intermediate tokens/DEXes policy allowlist;
- классифицировать provider errors без бесконечных retries;
- circuit-break при массовом schema drift или anomalous quotes;
- хранить raw response hash и request parameters без секретов.

Quote — рекомендация недоверенного внешнего сервиса, а не разрешение на подпись.

## 13. Policy engine

Policy evaluation детерминирована и не делает сетевых запросов. Все данные передаются в immutable input bundle. Решение содержит `allow/deny`, machine-readable reasons, policy version, input hash и expiry.

Минимальные правила:

- environment/cluster binding;
- global/tenant/strategy/wallet/mint kill switch;
- max atomic amount и USD notional;
- max slippage, price impact, platform/referral/priority fee;
- quote and blockhash freshness;
- token decimals, owner, freeze authority, Token-2022 extensions;
- program/DEX/ALT/account allowlist;
- expected signer/writable-account set;
- pre/post balance deltas и min-out;
- daily velocity и concurrent in-flight limit;
- provider agreement/skew;
- simulation success, logs denylist и compute headroom.

Изменение limit policy требует review, signed config artifact, canary и audit event. Повышение лимита не применяется к уже созданным approval bundles.

## 14. API contracts

### `POST /v1/intents`

Обязательные поля: `idempotencyKey`, `cluster`, `inputMint`, `outputMint`, `amountAtomic`, `maxSlippageBps`. В production также появятся tenant/strategy identity, deadline, allowed providers и client constraints.

Ответ `202` означает durable acceptance, а не execution success. Foundation возвращает `simulation_only`.

### `GET /v1/intents/{id}` (Stage 2)

Возвращает текущую проекцию, monotonic version, timestamps и безопасные reason codes. Raw provider payload и внутренние security details доступны только audit role.

### Error model

Ошибки имеют стабильные codes: `invalid_intent`, `duplicate_conflict`, `quote_unavailable`, `policy_denied`, `simulation_failed`, `expired`, `provider_degraded`, `reconciliation_required`. HTTP 5xx не сообщает клиенту, что сделка не состоялась; клиент обязан получить status по idempotency key.

## 15. Security и threat model

### 15.1 Активы

- signing keys и возможность инициировать подпись;
- средства и token accounts;
- intents/limits/policy configuration;
- canonical execution/audit history;
- API, RPC, Yellowstone и Jupiter credentials;
- availability confirmation/reconciliation path.

### 15.2 Угрозы и контрмеры

| Угроза                         | Контрмеры                                                                                                |
| ------------------------------ | -------------------------------------------------------------------------------------------------------- |
| Подмена intent/replay запроса  | Authn, tenant-bound idempotency, nonce/deadline, immutable revisions, signed service identity.           |
| Malicious quote/transaction    | Provider treated untrusted, independent decode/simulation, allowlists, min-out and balance-delta checks. |
| Compromised control API        | Signer accepts only policy-approved hash; network isolation; least privilege; no direct key access.      |
| Key exfiltration               | Remote HSM/KMS signer, no key in env/disk/logs, rotation, per-key caps, dual control.                    |
| RPC equivocation/stale state   | Multiple independent providers, context slots, slot-skew thresholds, finalized reconciliation.           |
| Yellowstone partial bank/gap   | Discard partial banks, replay, durable checkpoints, RPC backfill, parent linkage.                        |
| Duplicate execution            | Stable signature, idempotency state machine, no rebuild until proven expiry/non-landing.                 |
| Dependency/supply-chain attack | Lockfiles, pinned actions, provenance/SBOM, vulnerability/license scan, minimal images.                  |
| SSRF/credential leak           | Fixed provider endpoints, egress allowlist, secret redaction, no user-provided URLs.                     |
| DB tampering                   | Append-only audit, restricted roles, PITR, hash chain/periodic external anchoring.                       |
| DoS/rate exhaustion            | Quotas, bounded queues, per-provider budgets, backpressure, circuit breakers.                            |
| Insider raises limits          | Four-eyes approval, signed config, delayed activation, audit alert.                                      |

### 15.3 Secure defaults

- fail-closed при неизвестной программе, mint extension, policy field или provider response;
- secrets запрещены в git и structured logs;
- mTLS/workload identity между критическими сервисами;
- egress по allowlist;
- admin endpoints отделены от public API;
- production change требует reviewed artifact, а не ручной shell edit.

### 15.4 Foundation-specific controls

В Stage 1 отсутствуют signer, key env var и broadcast dependency. `ExecutionMode` в Rust имеет единственный вариант `Disabled`; TypeScript принимает только literal `disabled`. Это предотвращает случайное включение торговли конфигурацией.

## 16. Infrastructure

### 16.1 Environments

- local: Docker Compose PostgreSQL/Redis, disconnected market data;
- CI: ephemeral jobs и service containers;
- devnet: managed PostgreSQL/Redis, dedicated RPC/Yellowstone, no production keys;
- staging: mainnet read-only shadow traffic и synthetic/devnet signing only;
- production: отдельный cloud account/project, multi-AZ data services и isolated signer.

### 16.2 Production topology

- Kubernetes или эквивалентный orchestrator, минимум 3 zones;
- API autoscaling по RPS/latency; workers по queue lag;
- PostgreSQL multi-AZ, PITR, encrypted backups, connection proxy;
- Redis HA с `noeviction`, но без authoritative data;
- private subnets; ingress только через gateway; egress allowlist;
- object storage с versioning/retention lock для audit artifacts;
- infrastructure as code и immutable images по digest.

### 16.3 Resource/backpressure policy

Каждый worker имеет bounded concurrency и queue depth. При перегрузке API возвращает explicit `429/503` до durable acceptance. После `202` intent не теряется: очередь восстанавливается из PostgreSQL outbox. OOM/restart не сдвигает checkpoint до commit.

## 17. Observability и SLO

### 17.1 Metrics

- API: RPS, error codes, p50/p95/p99, idempotency conflicts;
- quote: latency/error/schema drift/price skew по provider/pair;
- ingestion: head slot, finalized lag, reconnects, partial banks, gap/backfill size;
- execution: intents по state, age, policy denial reasons, simulation failure/CU;
- signer: requests/denials/latency без message/key material;
- broadcast: attempts/channel latency/landing ratio/blockhash expiry;
- confirmation: processed→confirmed→finalized latency, fork/orphan count;
- DB/queue: connections, transaction latency, outbox lag, dead letters.

Запрещены high-cardinality labels: signature/intent/wallet помещаются в trace/log fields, но не metric labels.

### 17.2 Logs and traces

JSON logs содержат `trace_id`, `request_id`, `intent_id`, `revision`, `service`, `environment`, `provider`, `state_from/state_to` и safe reason code. Секреты, serialized signed transactions и полные balances по умолчанию редактируются. OpenTelemetry trace проходит через HTTP, outbox/event headers и signer request.

### 17.3 Alerts

Page: unauthorized signing attempt, global kill switch activation, finalized disagreement, signer errors, execution without approval evidence, RPO risk, database unavailable. Ticket: elevated quote latency, small backfill gap, single-provider degradation. Alert имеет runbook и подавляет производные симптомы.

### 17.4 SLI windows

Error-budget burn alerts: 2% за 1 час (fast) и 5% за 6 часов (slow) для availability/latency SLO. Execution integrity — zero-tolerance invariant, не процентный SLO.

## 18. Testing strategy

### 18.1 Unit/property tests

- state transition table и запрещённые переходы;
- integer amount/overflow/rounding boundaries;
- slippage/min-out/fee calculations;
- canonical serialization/hash stability;
- policy fail-closed на неизвестных полях;
- fork ancestry и compensation properties;
- fuzz instruction decoders и provider payload parsers.

### 18.2 Contract tests

- recorded Jupiter v2 fixtures, malformed/schema-drift/error fixtures;
- Yellowstone proto fixtures и reconnect/partial-bank sequences;
- Solana RPC error/null/pruned/rate-limit responses;
- Rust ↔ TypeScript JSON schema compatibility.

### 18.3 Integration tests

- PostgreSQL migration up from empty и forward upgrade from previous release;
- outbox atomicity и duplicate delivery;
- Redis loss/recovery без business data loss;
- service restart at every state boundary;
- RPC failover, timeout and disagreement;
- backfill joins live stream without gap or double projection.

### 18.4 End-to-end

- local validator deterministic fixtures;
- devnet quote→build→simulate→policy deny/approve with synthetic key;
- dropped transaction, expired blockhash, on-chain error, forked confirmed observation;
- kill switch during in-flight execution;
- disaster restore and replay to identical projection hash.

### 18.5 Performance and chaos

Load test 100 sustained/300 burst intents/s; 10k confirmation watchers; 1M-event replay. Chaos terminates workers, partitions Redis, delays RPC, truncates gRPC stream after partial bank, rate-limits Jupiter and fails one AZ. Acceptance checks both SLO and absence of duplicate business effect.

## 19. CI/CD и supply chain

### 19.1 Pull request gates

- format, lint, strict typecheck, unit/contract tests;
- Rust `fmt`, `clippy -D warnings`, tests and audit;
- SQL migration lint + empty/upgrade/reapply safety checks;
- dependency vulnerability/license scan, secret scan, SAST;
- container build, SBOM and critical-CVE policy;
- architecture/schema compatibility checks.

### 19.2 Artifact promotion

Один commit создаёт signed immutable images и SBOM. Тот же digest продвигается devnet → staging → production; rebuild между окружениями запрещён. Provenance подписывается keyless CI identity. GitHub environments требуют review для production.

### 19.3 Deployment

1. Expand-compatible DB migration.
2. Deploy readers/consumers, понимающие старую и новую схему.
3. Canary 5% read-only/shadow traffic.
4. Проверить SLO, projection parity и security invariants.
5. Постепенно увеличить traffic.
6. Contract/remove migration — отдельным релизом после rollback window.

Workers draining: перестают брать новые leases, завершают транзакцию БД, сохраняют checkpoint и выходят. Rollback приложения не откатывает destructive schema changes.

## 20. Backup, disaster recovery и data retention

- PostgreSQL PITR минимум 35 дней; encrypted daily snapshot минимум 90 дней;
- ежемесячный restore drill с измерением RTO и projection hash;
- audit events и execution evidence — минимум 7 лет либо по применимой политике;
- raw market payloads — tiered retention, минимум 30 дней hot/180 дней archive;
- Redis backup не требуется для correctness;
- signer configuration/keys имеют независимый backup/rotation procedure без экспорта private material.

После restore ingestion запускается read-only, выполняет backfill/reconciliation, сравнивает terminal signatures, затем readiness разрешает новые intents. Подпись остаётся disabled до ручного DR approval.

## 21. Tradeoffs

### Rust + TypeScript против одного языка

Два toolchain усложняют CI и контракты, но отделяют быстрый product/control plane от memory-safe, детерминированного data plane. На границе обязательны schema/contract tests.

### PostgreSQL outbox против Kafka с первого дня

PostgreSQL уменьшает операционную сложность и даёт атомарность intent+event. При доказанном throughput bottleneck outbox можно публиковать в Kafka/NATS без смены source of truth.

### Yellowstone + RPC против одного источника

Yellowstone снижает latency, RPC обеспечивает portable backfill и независимую проверку. Цена — reconciliation complexity; она необходима для execution integrity.

### Jupiter Router против managed execute

Router лишает engine части RFQ/managed landing преимуществ, но даёт проверяемые instructions и собственный broadcast. Managed path может быть отдельным adapter с более строгой trust policy.

### Finalized accounting против confirmed UX

Finalized увеличивает terminal latency, но исключает большую часть fork ambiguity. UI может показывать confirmed progress, не меняя окончательный accounting.

## 22. Реестр рисков

| Риск                            | Вероятность/влияние | Митигация                                              | Trigger                   |
| ------------------------------- | ------------------- | ------------------------------------------------------ | ------------------------- |
| Jupiter API/schema changes      | Средняя/Высокое     | Version adapter, fixtures, schema circuit breaker      | parse failure > 0.1%      |
| RPC/Yellowstone disagreement    | Средняя/Высокое     | multi-provider quorum, quarantine, finalized reconcile | slot/hash mismatch        |
| Duplicate economic effect       | Низкая/Критическое  | signature idempotency, expiry proof, state machine     | second signature/revision |
| Compromised signer path         | Низкая/Критическое  | HSM, policy-bound hash, network isolation, caps        | any unauthorized request  |
| Token-2022 unexpected semantics | Средняя/Высокое     | extension deny-by-default, explicit adapters           | unknown extension         |
| Congestion/priority fee spike   | Высокая/Среднее     | fee caps, expiry, abort instead of chase               | fee cap exceeded          |
| DB outage/backlog               | Средняя/Высокое     | HA/PITR, bounded intake, outbox replay                 | queue/SLO burn            |
| Provider retention too short    | Средняя/Высокое     | archive RPC, continuous gap monitor                    | pruned needed slot        |
| Operational limit mistake       | Средняя/Критическое | reviewed signed config, canary, delay                  | limit increase            |
| Regulatory/custody scope        | Неизвестна/Высокое  | non-custodial boundary, legal review before mainnet    | custody/business change   |

## 23. Измеримые acceptance criteria

### Stage 1 Foundation — текущий этап

- отдельный Git repository; `git status` Sentinel остаётся чистым;
- Rust workspace и TypeScript workspace собираются без warnings/errors;
- unit/API tests проходят; lint/typecheck/format зелёные;
- Docker Compose config валиден; PostgreSQL и Redis достигают healthy ≤ 60 секунд;
- миграция создаёт schema в пустой БД без ошибки;
- все health endpoints отвечают 200;
- `EXECUTION_MODE=live` вызывает startup/config failure;
- repository search подтверждает отсутствие private key variable, signer и `sendTransaction` path;
- README, SDD и ADR доступны в репозитории.

### Stage 2 Read-only market/quote foundation

- 24 часа непрерывного devnet/mainnet read-only ingestion без необъяснённого gap;
- reconnect с искусственным partial bank даёт идентичный projection hash;
- RPC backfill 100k slots сходится с live ledger, 0 пропущенных execution-related facts;
- Jupiter quote-only contract suite и rate-limit/circuit-break tests зелёные;
- p99 finalized projection lag ≤ 3 slots при нормальном провайдере;
- signing/broadcast всё ещё отсутствуют.

### Stage 3 Simulation and policy

- 100% instruction/account/program set декодируется либо fail-closed;
- 10k recorded swaps replay дают deterministic policy outcome;
- property/fuzz suite ≥ 10M cases без panic/overflow;
- policy p99 ≤ 20 ms;
- mainnet shadow simulation ≥ 7 дней, unexplained balance delta = 0.

### Stage 4 Devnet execution

- отдельный synthetic signer, per-tx cap и kill switch;
- ≥ 10k devnet executions, duplicate business effects = 0;
- ≥ 99% landing при нормальных условиях в заданный expiry budget;
- fork/expiry/restart/partition chaos suite зелёный;
- signer requests без complete approval chain = 0.

### Stage 5 Limited mainnet canary

- независимый security review закрыт;
- DR restore и key rotation drills пройдены;
- 30 дней staging SLO, 0 integrity incidents;
- лимиты: одна разрешённая стратегия/пара/кошелёк, минимальный notional и дневной cap;
- manual approval и global kill switch на canary;
- расширение только после post-canary review, не автоматически.

## 24. Milestones

| Этап              | Содержание                                                               | Exit decision                        |
| ----------------- | ------------------------------------------------------------------------ | ------------------------------------ |
| M1 Foundation     | Monorepo, core contracts, fail-closed services, DB/Redis, CI, SDD/ADR    | Локальные проверки и CI green        |
| M2 Observe        | Yellowstone/RPC adapters, canonical ledger, backfill, Jupiter quote-only | 24h soak + gap/replay acceptance     |
| M3 Decide         | Build/decode, simulation, policy, operator kill switches                 | Determinism/security/load acceptance |
| M4 Devnet act     | Isolated signer, broadcast, confirmer, devnet only                       | 10k runs + chaos + audit review      |
| M5 Mainnet shadow | Mainnet read/simulate, no sign                                           | 30d parity/SLO                       |
| M6 Mainnet canary | Hard caps, manual approval, one strategy                                 | Formal go/no-go review               |
| M7 Scale          | More pairs/strategies, automation under limits                           | Per-expansion risk review            |

## 25. Runbooks, обязательные до production

- provider outage/rate limit;
- Yellowstone reconnect, partial banks и retention overflow;
- RPC disagreement/fork incident;
- stuck signed transaction и blockhash expiry;
- kill switch and safe resume;
- signer compromise/rotation;
- database failover/PITR restore;
- Jupiter schema drift/anomalous quote;
- unexpected Token-2022 extension/program upgrade;
- reconciliation after deployment rollback.

Safe resume всегда требует: устранённую причину, closed gap, terminal signature reconciliation, здоровые SLO и явное снятие kill switch другим уполномоченным лицом.

## 26. Открытые решения перед Stage 3+

1. Конкретный HSM/KMS/remote signer и поддержка Ed25519 с policy enforcement.
2. RPC/Yellowstone providers, независимость инфраструктуры и archive retention.
3. Jupiter Router `/build` против отдельного managed `/order` adapter для некоторых стратегий.
4. Jito/TPU/RPC multi-broadcast policy и защита от privacy/MEV leakage.
5. Token allowlist, USD price source и stale-price quorum.
6. Юридические/custody ограничения и audit retention by jurisdiction.
7. Требуемый RPS/latency после подтверждённого product workload.

Ни один из этих вопросов не мешает Foundation и read-only Stage 2, но каждый блокирует real-money execution.

## 27. Источники и проверенные внешние допущения

- [Solana RPC overview and commitments](https://solana.com/docs/rpc) — commitment semantics и предупреждение о public endpoints.
- [Solana HTTP RPC methods](https://solana.com/docs/rpc/http) — block, signature, simulation и blockhash methods.
- [Solana `blockSubscribe`](https://solana.com/docs/rpc/websocket/blocksubscribe) — subscription semantics и unstable status.
- [Yellowstone gRPC repository](https://github.com/rpcpool/yellowstone-grpc) — Geyser stream implementation и clients.
- [Yellowstone changelog](https://github.com/rpcpool/yellowstone-grpc/blob/master/CHANGELOG.md) — reconnect replay, partial banks и version changes.
- [Yellowstone protobuf](https://github.com/rpcpool/yellowstone-grpc/blob/master/yellowstone-grpc-proto/proto/geyser.proto) — filters, commitments и `from_slot`.
- [Jupiter Swap API v2](https://developers.jup.ag/docs/swap) — `/order`, `/execute`, `/build`, routing и migration from v1.

Внешние API быстро меняются. Ссылки фиксируют состояние дизайна на дату документа; production adapter обязан pin-ить протестированную версию и обнаруживать drift автоматически.
