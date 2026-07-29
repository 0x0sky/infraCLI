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
infra rm [path]
infra <service>
infra <service> logs
infra <service> restart
infra <service> stop
infra <service> rm
```

`infra` shows project state. `infra conf` creates configuration. `infra apply` reconciles runtime state.

`infra stop` stops managed containers but keeps them available for a later start.

`infra rm` removes managed runtime containers after confirmation. It does not remove `.infra`, images, volumes, networks, or source files.

`infra conf` and `infra conf .` both target `./.infra`. A directory path becomes `<path>/.infra`; an explicit `.infra` or `*.infra` filename is preserved.

## lifecycle semantics

```text
conf → apply → inspect → stop | restart | rm
```

- `stop` preserves the container object.
- `restart` recreates the selected service from the current `.infra` desired state using the existing image.
- `rm` removes the selected managed container after explicit confirmation.
- a later `apply` recreates runtime objects that are still declared in `.infra`.

Project-level removal:

```text
infra rm
remove project market runtime objects? [y/n]:
```

Service-level removal:

```text
infra api rm
remove service api runtime object? [y/n]:
```

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
