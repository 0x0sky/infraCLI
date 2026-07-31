# infraCLI

`infra` is a small Rust tool for describing, reconciling, and observing container infrastructure through one `.infra` file.

```text
project/
├── .infra
└── Dockerfile
```

Docker is the first runtime backend. The configuration and command model remain runtime-independent.

## commands

```text
infra
infra conf [path] [-a|--apply]
infra apply [path]
infra stop [path]
infra rm [--path <path>]
infra rm -a|--all [--path <path>]
infra rm <service> [--path <path>]
infra auth telegram --endpoint <https-url> --source <id> [--no-open]
infra agent [--config <path>] [--once]
infra <service>
infra <service> logs
infra <service> restart
infra <service> stop
```

`infra` shows project state. `infra conf` creates configuration. `infra apply` reconciles runtime state.

`infra stop` stops managed containers but keeps their runtime objects.

## containerized service monitor

`infra agent` is a long-running monitoring process intended to run in its own hardened container. It does not own the monitored application containers. Its `.infra` block declares where observations come from and which Telegram bot receives normalized state-change events.

```text
agent {
    poll_interval_seconds = 15
    health_bind = "0.0.0.0:9090"
    state_file = "/var/lib/infra/state.json"

    input "market-docker" {
        driver = "docker"
        addresses = ["http://docker-api:2375"]
        projects = ["zero-x-da-market-development", "zero-x-da-market-bot-development"]
        services = ["api", "bot"]
        fields = ["input", "docker_address", "container", "project", "service", "image", "state", "health"]
    }

    output "infra_services_bot" {
        driver = "infrabot"
        endpoint = "http://infra-bot:8787"
        source = "vps-spaceship-01"
        credential = env("INFRA_CREDENTIALS_FILE")
        events = ["service.failed", "service.recovered", "service.started", "service.removed"]
        fields = ["kind", "project", "service", "container", "image", "previous_status", "status"]
    }
}
```

The `output` block name is the Telegram bot username. `driver = "infrabot"` selects its delivery backend and API protocol; it is not the bot identity.

The first poll establishes a baseline and sends nothing. Later polls compare persistent state and emit only transitions. A failed delivery does not advance the stored snapshot, so the transition is retried on the next poll.

`input.fields` is the extraction boundary for Docker observations. Raw Docker fields not declared there cannot appear in an event. `output.fields` is the projection rendered by the named Telegram bot. Derived transition fields such as `kind`, `previous_status`, and `status` are produced by the agent.

Version 1 supports multiple Docker inputs and exactly one Telegram bot output. Every event includes the output name, and infraBot rejects an event when that name does not match its configured Telegram bot.

Docker inputs use restricted HTTP API addresses. Do not mount the raw Docker socket into the agent container: access must pass through a private, read-only Docker API proxy exposing only the endpoints required for container listing.

The provided `Dockerfile.agent` runs as uid/gid `65532`, stores state under `/var/lib/infra`, exposes health on port `9090`, and starts:

```bash
infra agent --config /etc/infra/.infra
```

Use `examples/infra-agent.infra` as the complete contract.

## Telegram authorization

Each agent or CLI installation authorizes one source ID declared in infraBot's `.infra` registry:

```bash
INFRA_CREDENTIALS_FILE=/run/secrets/infrabot.json \
infra auth telegram \
  --endpoint https://<infrabot-host> \
  --source vps-spaceship-01 \
  --no-open
```

Environment variables may replace endpoint and source:

```bash
INFRABOT_URL=https://<infrabot-host> \
INFRA_SOURCE=vps-spaceship-01 \
infra auth telegram
```

The command creates a short-lived pairing session, generates a private verifier locally, sends only its SHA-256 challenge, opens or prints the Telegram deep link, and waits for approval. The resulting token is returned only to the CLI holding the verifier.

The deep link contains neither the token nor verifier. Production authorization endpoints require HTTPS; HTTP is accepted only for exact localhost hosts.

Credentials are written atomically to `${XDG_CONFIG_HOME}/infra/credentials.json`, `~/.config/infra/credentials.json`, or `INFRA_CREDENTIALS_FILE`. On Unix, managed directories use mode `0700` and credential files mode `0600`.

## lifecycle semantics

```text
conf → apply → inspect → stop | restart | rm
```

- `stop` preserves the container object.
- `restart` recreates the selected service from current `.infra` desired state using the existing image.
- `rm` without a target removes `.infra` and deinitializes infra for the project. It does not remove running services.
- `rm -a` or `rm --all` removes every managed service while preserving `.infra`.
- `rm <service>` removes one managed service while preserving `.infra`.
- a later `apply` recreates services that remain declared in `.infra`.

Deinitialize infra:

```text
infra rm
deinitialize infra at ./.infra? [y/n]:
```

Remove every managed service without deinitializing:

```text
infra rm --all
remove all services from project market? [y/n]:
```

Remove one service:

```text
infra rm api
remove service api? [y/n]:
```

The three forms are intentionally distinct. `infra rm` only owns configuration lifecycle; service removal requires an explicit service target or `--all`.

`infra conf` and `infra conf .` both target `./.infra`. A directory path becomes `<path>/.infra`; an explicit `.infra` or `*.infra` filename is preserved.

## configuration ownership

The standard `project` block belongs to infraCLI. Monitoring or application blocks after it belong to the process consuming them:

```text
infra 1

project "infra-agent" {
    runtime = "docker"

    service "agent" {
        source = "."
        build = "Dockerfile.agent"
        expose = 9090
        health = "/health"
    }
}

agent {
    # parsed by infra agent
}
```

infraCLI preserves trailing application-owned blocks when `infra conf` rewrites a file. The core runtime model stays independent from Telegram, notification providers, and application-specific routing.

The service `environment` field is passed to Docker as `--env-file`. Secrets therefore remain outside `.infra`; extension blocks reference them with `env("NAME")`.

## configuration flow

Interface copy is lowercase. User-provided values retain their original case.

```text
project name [market]:
runtime [docker]:
service source [.]:
use detected values for the remaining fields? [y/n]:
```

After three accepted defaults, the wizard offers auto-fill. Final review:

```text
write configuration? [y/e/n]:
```

- `y` writes the file.
- `n` exits without writing.
- `e` starts with the first field not entered manually.

During edit mode, current auto-filled values use parentheses. Pressing Enter preserves the current value.

## `.infra`

```text
infra 1

project "market" {
    runtime = "docker"

    service "api" {
        source = "."
        build = "Dockerfile"
        expose = 8080
        health = "/health"
        environment = ".env"
    }
}
```

The initial runtime model supports one service and Docker. Configuration parsing, CLI commands, runtime execution, and application extensions remain separated so additional services, inputs, outputs, and runtimes can evolve without changing lifecycle contracts.

## development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
docker build -f Dockerfile.agent -t infra-agent:local .
```
