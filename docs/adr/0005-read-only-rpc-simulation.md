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
- Ответ всегда содержит `executionAllowed=false`. Успешная симуляция не считается policy decision, approval evidence или разрешением на подписание.
- Не добавляются приватные ключи, signer, transaction builder, Jupiter `/execute` или `sendTransaction`.

## Компромиссы и последствия

Транзакцию может симулировать RPC без проверки signature validity, поэтому результат подтверждает только поведение программы в контексте симуляции. Текущий endpoint не декодирует message, инструкции, program IDs, writable accounts или Address Lookup Tables. До следующего продвижения требуется полный version-aware decoder, fail-closed allowlist и ALT resolution. Все Stage 3 acceptance gates остаются открытыми.

## Проверка

Автотесты проверяют RPC method/options, допустимый размер/encoding и инвариант `executionAllowed=false`. Они не заменяют тесты против выделенного provider, instruction-decoder suite, fuzzing, soak или независимый security review.
