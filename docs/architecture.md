# Architecture

The CLI is a single Rust package that emits one `rosco` executable. Internally,
the code is separated by change reason: command-line presentation, workflow
orchestration, host adapters, and transfer protocol.

```text
src/main.rs + cli.rs          parse command and render top-level errors
             |
             v
          app.rs              build/upload/monitor/run use cases
          /    \
         v      v
     build.rs  serial.rs      host process and UART adapters
                  |
                  v
              kermit.rs       transport protocol and packet state machine

          config.rs           project policy consumed by the use cases
```

## Module responsibilities

- `cli` owns the stable user-facing command and option schema. It performs no
  hardware or filesystem work.
- `app` resolves precedence (`CLI > rosco.toml > defaults`) and composes the
  use cases. In particular, `run` keeps one serial port open across upload and
  monitoring so there is no reconnect race.
- `build` prompts for Docker or a local toolchain. Docker mounts the project in
  the configured image; the local path checks and, after confirmation, prepares
  the Homebrew toolchain. Both invoke the configured builder and verify its
  declared artifact. The module does not know Makefile syntax or m68k compiler
  details.
- `serial` owns host-specific serial discovery/opening behind `serialport`'s
  cross-platform trait. All connections are 8-N-1 with no flow control so
  binary upload remains 8-bit clean.
- `kermit` owns packet encoding, negotiation, checksums, retries, and transfer
  progress. It has no CLI or UI dependency.
- `config` validates `rosco.toml` and rejects unknown keys so typos do not fail
  silently.

## Core flow

```text
rosco run
  -> load configuration
  -> choose Docker or local toolchain
  -> execute builder (Docker mounts the project at /workspace)
  -> verify the .bin artifact
  -> resolve/open UART
  -> Kermit S -> F -> D... -> Z -> B
  -> interactive UART session (stdin/stdout with terminal raw mode)
```

## Portability model

There are two different architectures involved:

1. The `rosco` host binary is compiled natively for Linux/Windows and
   x86_64/AArch64. Platform details stay inside dependencies and host adapters.
2. The user's bare-metal application is compiled for Motorola 68k either in
   the configured Docker image (the recommended, reproducible path) or by the
   local Homebrew toolchain after an explicit user confirmation. The CLI never
   assumes host and target architectures are the same.

Native CI runners cover the four promised host combinations. The serial crate
is built without its optional Linux `libudev` feature, avoiding a dynamic
system-library dependency and keeping distribution simpler.

## Planned extension points

- A project initializer can generate C/assembly templates without changing the
  build or serial layers.
- A toolchain service can select or pull Docker toolchains, then feed the
  resulting image into the existing build adapter.
- Additional upload protocols can implement the upload use case beside Kermit.
- An emulator adapter can become another `run` destination.
- Interactive UART input and terminal raw mode can extend `serial` while the
  protocol remains unchanged.

These should be added when their user-facing behavior is defined; the current
boundaries are intentionally small enough to evolve without a plugin framework.
