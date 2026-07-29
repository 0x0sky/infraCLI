# Notifications

> Status: proposed architecture. The `.infra` notification configuration, event bus, watcher commands, persistent state, and providers described here are not implemented yet.

`infra` is intended to support an event-driven notification layer independent from the runtime implementation.

The notification system receives normalized events from `infra core`, applies delivery policy, and sends notifications through configured providers such as Telegram, email, Slack, or webhooks.

## Goals

- notify about infrastructure failures;
- notify about successful recovery;
- deliver only meaningful state changes;
- deduplicate repeated alerts;
- preserve enough state to calculate downtime across watcher restarts;
- keep notification policy independent from Docker, Podman, or another runtime;
- make providers replaceable without changing monitoring logic.

## Non-goals

- providers do not inspect containers or runtime state directly;
- providers do not decide health transitions or severity;
- notification configuration does not contain secret values;
- in-host monitoring does not replace an external availability monitor.

## Configuration

```infra
infra 1

project "market" {
    runtime = "docker"

    notifications {
        telegram {
            token   = env("INFRA_TELEGRAM_BOT_TOKEN")
            chat_id = env("INFRA_TELEGRAM_CHAT_ID")
        }
    }

    service "api" {
        source = "."
        build  = "Dockerfile"
        expose = 8080
        health = "/health"
    }
}
```

Secrets are never stored inside `.infra` files. Configuration stores only references to environment variables or another future secret source.

A provider configuration must fail validation when a required reference is missing, but validation must never print the resolved secret value.

## Event model

Runtime adapters and monitors produce normalized events. Notification code consumes those events and never calls Docker, Podman, systemd, or another runtime directly.

Every event should include a stable envelope:

```text
Event
├── id
├── kind
├── occurred_at
├── project
├── target
├── subject
├── severity
├── fingerprint
└── details
```

The `fingerprint` identifies one logical alert stream, for example:

```text
market:vps-spaceship-01:service:api:health
```

It is used for transition tracking, deduplication, cooldowns, retry idempotency, and recovery correlation.

## Supported events

### Service lifecycle

- `ServiceStarted`
- `ServiceStopped`
- `ServiceRestarted`
- `ServiceRemoved`

### Health

- `ServiceHealthy`
- `ServiceUnhealthy`
- `ServiceRecovered`

`ServiceHealthy` represents an initially observed healthy service. `ServiceRecovered` is emitted only for an `unhealthy -> healthy` transition and carries the beginning and duration of the outage when known.

### Runtime

- `RuntimeUnavailable`
- `RuntimeRecovered`

### Build

- `BuildStarted`
- `BuildSucceeded`
- `BuildFailed`

### Apply

- `ApplyStarted`
- `ApplySucceeded`
- `ApplyFailed`

### Configuration

- `ConfigurationCreated`
- `ConfigurationRemoved`

Event names are runtime-neutral. A Docker container restart, a Podman container restart, and a future process-runtime restart all normalize to the same `ServiceRestarted` event.

## Notification pipeline

```text
runtime adapter / monitor
          │
          ▼
   normalized Event
          │
          ▼
       event bus
          │
          ▼
 notification policy
 ├── state transitions
 ├── severity mapping
 ├── deduplication
 ├── cooldown
 └── rendering
          │
          ▼
    delivery queue
 ├── retries
 ├── backoff
 └── idempotency
          │
          ▼
 configured providers
```

The event bus carries facts. The policy layer decides whether a fact should produce a user-visible notification. Providers are transport adapters only.

## Telegram message example

```text
🔴 infra · service unhealthy

project : market
service : api
host    : vps-01

health check failed

GET /health
connection refused
```

## Recovery message

```text
🟢 infra · service recovered

project : market
service : api

downtime : 2m 14s

state : healthy
```

## Notification policy

The notification layer should:

- deduplicate repeated failures by fingerprint;
- notify only on relevant state changes;
- support configurable cooldowns;
- retry failed deliveries;
- classify events by severity;
- preserve transition state across process restarts;
- correlate recovery with the failure that opened the incident;
- redact secrets and sensitive headers from event details.

| Severity | Typical events |
| --- | --- |
| `info` | started, stopped, initial healthy state |
| `warning` | restart, temporary health issue |
| `error` | build failed, apply failed, sustained unhealthy state |
| `critical` | runtime unavailable, repeated or multi-service failure |

The default policy should not send the same open failure on every health-check interval. Repeated observations may increase counters or severity internally, but another notification is sent only when policy allows escalation or a configured cooldown expires.

Recovery bypasses the failure cooldown: when an open failure returns to a healthy state, the recovery notification is sent immediately.

## Persistent state

Deduplication and downtime calculation require state that survives watcher restarts.

The notification subsystem therefore needs a `NotificationStateStore` containing, at minimum:

- the last known state for each fingerprint;
- when the current failure started;
- the last notification attempt and successful delivery;
- the current severity and repeat count;
- provider-specific delivery identifiers when available.

The storage implementation is an internal concern. Notification policy depends on the state-store interface, not on a particular database or file format.

## Delivery semantics

Delivery is at least once. A provider request may succeed even when its response is lost, so each delivery carries a stable idempotency key derived from the event and provider.

Failed deliveries use bounded exponential backoff with jitter. Exhausted deliveries are retained in logs or a future dead-letter store; they must not block monitoring or event processing.

A provider failure is not an infrastructure recovery and must never alter the monitored service state.

## CLI

The following commands are reserved for the notification and monitoring implementation.

### Test notification

```bash
infra notify test
```

Sends a synthetic test notification through every configured provider and reports each delivery result independently.

### One-shot health check

```bash
infra watch --once
```

Performs one monitoring cycle.

Suitable for:

- cron;
- a systemd timer;
- CI;
- an external monitoring node.

A one-shot process needs the same persistent state store as continuous monitoring; otherwise it cannot reliably deduplicate failures or calculate recovery duration between invocations.

### Continuous monitoring

```bash
infra watch
```

Runs continuously and:

- observes configured services;
- performs health checks;
- emits normalized infrastructure events;
- updates transition state;
- dispatches eligible notifications.

The monitor emits events. It does not call a Telegram, email, or Slack provider directly.

## Failure-domain boundary

A watcher running on `vps-spaceship-01` can detect a failed service or an unavailable local container runtime while the host and watcher process remain alive.

It cannot report total host loss, power loss, kernel failure, or loss of all outbound connectivity from that same host. Those conditions require an external watcher on a separate failure domain. The internal `infra watch` process and an external uptime monitor are complementary rather than interchangeable.

## Architecture

```text
infra
│
├── cli
│
├── core
│   ├── planner
│   ├── runtime
│   ├── monitor
│   └── event bus
│
└── notifications
    ├── policy
    ├── state store
    ├── delivery queue
    ├── renderer
    └── providers
        ├── telegram
        ├── email
        ├── slack
        └── webhook
```

The notification subsystem depends only on normalized events and notification contracts. It never communicates directly with Docker, Podman, or another runtime.

## Provider contract

Providers receive an already classified and rendered notification. They do not own monitoring, state transitions, cooldowns, or message policy.

```rust
pub trait NotificationProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn send(
        &self,
        notification: &Notification,
    ) -> Result<DeliveryReceipt, NotificationError>;
}
```

This boundary allows provider-specific transport behavior without coupling the rest of the system to provider APIs.

## Planned providers

- Telegram
- Email
- Slack
- Discord
- Webhook
- Apple Push Notification Service (APNs)
- Matrix

All providers implement the same delivery contract. Adding a provider must not change runtime adapters, health checks, event definitions, or notification policy.
