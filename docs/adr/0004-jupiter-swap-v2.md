# ADR-0004: Jupiter Swap API v2 как первый quote adapter

- Статус: принят (read-only в Foundation)
- Дата: 2026-11-11

## Контекст

Jupiter объявляет Metis Swap API v1 устаревшим и рекомендует Swap API v2. V2 предлагает Meta-Aggregator `/order` + `/execute` и Router `/build` + самостоятельную отправку.

## Решение

На Foundation разрешены только quote-only запросы `GET /swap/v2/order` без `taker`. Для будущего контролируемого data plane выбираем Router `/build`, потому что engine должен независимо декодировать, симулировать и проверять инструкции до подписи. `/execute` не используется до отдельного security review: managed landing уменьшает контроль и может включать RFQ/last-look semantics.

## Последствия

В краткосрочной перспективе нет лучшей цены от всех RFQ-источников в собственной execution pipeline. Взамен сохраняются прозрачность инструкций, собственные лимиты, симуляция и несколько каналов broadcast.
