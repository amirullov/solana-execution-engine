# ADR-0005: Read-only RPC simulation boundary

- Статус: принят для simulation foundation; не является разрешением на execution
- Дата: 2026-11-12

## Контекст

Нужно получить начальную runtime-симуляцию сериализованной Solana транзакции, не вводя signer или broadcast path. RPC `simulateTransaction` принимает base64-транзакцию, поддерживает `sigVerify=false` и `replaceRecentBlockhash=true`.

## Решение

- Execution Engine использует только `SOLANA_RPC_URL` из конфигурации; клиент не может передать URL.
- Запрос принимает canonical base64 до 4 KiB, имеет HTTP body cap 8 KiB и RPC timeout 10 секунд.
- RPC вызов всегда использует commitment `confirmed`, `sigVerify=false`, `replaceRecentBlockhash=true`.
- RPC response ограничен 1 MiB; число и длина возвращаемых log lines ограничены.
- До RPC pinned `agave-transaction-view = 10.1.0` sanitizes legacy/v0/v1 структуры и собирает static keys, program IDs, account references и instruction data.
- Неизвестный/повреждённый формат отклоняется. v0 с ALT отклоняется до реализации полного ALT resolution. Для v1 обязательны положительные `computeUnitLimit` и `loadedAccountsDataSizeLimit`.
- Ответ всегда содержит `executionAllowed=false`. Успешная симуляция не считается policy decision, approval evidence или разрешением на подписание.
- Не добавляются приватные ключи, signer, transaction builder, Jupiter `/execute` или `sendTransaction`.

## Компромиссы и последствия

Транзакцию может симулировать RPC без проверки signature validity, поэтому результат подтверждает только поведение программы в контексте симуляции. Текущий parser структурно читает instructions, но не декодирует их по DEX/Token program semantics, не проверяет writable-account policy и не разрешает ALTs. До следующего продвижения требуются semantic decoders, allowlist, ALT resolution, полный risk policy и approval evidence. Все Stage 3 acceptance gates остаются открытыми.

## Проверка

Автотесты проверяют legacy/v0/v1 parsing, malformed input rejection, отказ при ALT и неполных v1 limits, RPC method/options и инвариант `executionAllowed=false`. Они не заменяют тесты против выделенного provider, program-specific decoder suite, fuzzing, soak или независимый security review.
