# infraCLI

`infra` is a small Rust CLI for describing and reconciling a project's container infrastructure through one `.infra` file.

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
infra <service>
infra <service> logs
infra <service> restart
infra <service> stop
```

`infra` shows project state. `infra conf` creates configuration. `infra apply` reconciles runtime state.

`infra stop` stops managed containers but keeps their runtime objects.

## Telegram authorization

Each CLI installation authorizes one source ID declared in infraBot's `.infra` registry:

```bash
infra auth telegram \
  --endpoint https://<infrabot-host> \
  --source primary
```

Environment variables may replace both flags:

```bash
INFRABOT_URL=https://<infrabot-host> \
INFRA_SOURCE=primary \
infra auth telegram
```

Use `--no-open` on a headless host. The one-time Telegram deep link is always printed.

The command creates a short-lived pairing session, generates a private verifier locally, sends only its SHA-256 challenge, opens the Telegram deep link, and waits for approval. The resulting access token is returned only to the CLI that holds the verifier.

One infraBot instance and one Telegram bot may authorize multiple declared sources:

```text
infraCLI · primary ─┐
                    ├──► one infraBot ───► one Telegram bot
infraCLI · secondary┘
```

Every source receives an independent session, verifier, signed token, and credential file. Pairing `secondary` does not replace or invalidate `primary`. The source ID is included in the token and must still exist in infraBot's registry when the token is used.

The deep link contains neither the access token nor the verifier. Production endpoints require HTTPS; HTTP is accepted only for exact localhost hosts.

Credentials are written atomically to `${XDG_CONFIG_HOME}/infra/credentials.json` or `~/.config/infra/credentials.json`. The document contains the infraBot endpoint, source ID, token type, access token, expiry, and approving Telegram user ID. On Unix, the managed directory is mode `0700` and the credential file is mode `0600`. Set `INFRA_CREDENTIALS_FILE` to use another protected location.

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

The standard `project` block belongs to infraCLI. Application-owned blocks after it belong to the application that consumes them:

```text
infra 1

project "infrabot" {
    runtime = "docker"

    service "api" {
        source = "."
        build = "Dockerfile"
        expose = 8787
        health = "/health"
        environment = ".env"
    }
}

infrabot {
    # owned and parsed by infraBot
}
```

infraCLI preserves trailing application-owned blocks when reading and rendering `.infra`. When `infra conf` rewrites an existing file, it carries those blocks forward instead of interpreting or discarding them. This keeps the core runtime-independent and prevents Telegram-specific configuration from leaking into infraCLI's domain model.

The `environment` field is passed to Docker as `--env-file`. Secrets therefore remain outside `.infra`; application blocks should reference them through their own environment contract.

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

During edit mode, current auto-filled values use parentheses:

```text
build file (Dockerfile):
edit remaining fields? [y/n]:
```

Pressing Enter preserves the current value.

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

The initial runtime model supports one service and Docker. Configuration parsing, CLI commands, runtime execution, and application extensions remain separated so additional services and runtimes can be added without changing lifecycle contracts.

## development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
