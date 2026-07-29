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
infra <service>
infra <service> logs
infra <service> restart
infra <service> stop
```

`infra` shows project state. `infra conf` creates configuration. `infra apply` reconciles runtime state.

`infra stop` stops managed containers but keeps their runtime objects.

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

The initial MVP supports one service and a Docker backend. The architecture keeps configuration, CLI parsing, and runtime execution separated so multiple services and runtimes can be added without changing the lifecycle contract.

## development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
