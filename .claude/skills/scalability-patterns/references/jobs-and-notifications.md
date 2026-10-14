# Jobs and notifications

Covers durable background jobs (deploy, snapshot, image pull) and delivering events (deploy finished, node offline, quota warnings) to users and integrations via in-app, email, webhooks, and chat.

## Job pipeline
1. API validates the request, writes **desired state and a job row** in one transaction, returns `202 Accepted` with the job id.
2. A dispatcher publishes the job to a queue (transactional outbox: write the outbox row in the same transaction, relay it afterward, so no job is lost or duplicated by a crash between commit and publish).
3. Workers pull, take a per-resource lease, execute, and record result and progress.
4. Failures: classify as **retryable** (timeouts, node busy) or **permanent** (validation, auth). Retry with exponential backoff + jitter; cap attempts; dead-letter and alert after the cap.
5. Cancellation and timeouts are first-class; leases expire so a crashed worker's job is picked up again.

Design rules: idempotent handlers, one active job per resource, ordered by generation number, visible state machine, and every transition logged and measured.

## Notification system
- **Producers** emit events (`deploy.succeeded`, `node.offline`). **Notification service** resolves recipients and preferences, renders a template, and enqueues one message per channel.
- **Channels** (email, webhook, chat, in-app) each have their own queue and workers so a slow provider does not block others. Third-party senders are behind an interface so providers can be swapped.
- **Reliability:** persist notifications before sending; at-least-once with retries; **deduplicate** by event id + recipient + channel so users do not get duplicates; dead-letter after limits.
- **User controls:** per-channel opt-in/out, quiet hours, digesting; rate-limit sends per recipient (`rate-limiter.md`).
- **Webhooks:** sign payloads (HMAC with per-endpoint secret), include timestamp and event id, retry with backoff, disable endpoints that fail persistently, and block requests to private IP ranges (SSRF protection).
- **Security:** do not put secrets or sensitive config in messages; templates escape user-provided values.
- **Observability:** queue depth, send latency, success/failure per channel, bounce rate.

## Tests
- Crash between commit and publish (outbox recovers)
- Duplicate delivery does not repeat side effects
- Retry/backoff behavior and dead-letter path
- Webhook signature verification and SSRF rejection
