# Architecture

The CLI is a single Rust package that emits one `rosco` executable. Internally,
the code is separated by change reason: command-line presentation, workflow
orchestration, host adapters, and transfer protocol.

```text
src/main.rs + cli.rs        parse command and render top-level errors
        |
        v
      app.rs                init/build/upload/monitor/run use cases
     /    |    \
    v     v     v
init.rs  build.rs  serial.rs, emulator.rs
prompt.rs   |          |
            v          v
     toolchain.rs   kermit.rs, ihex.rs
     compilers and  one transfer protocol
     what installs  per board
     them

      config.rs             project policy consumed by the use cases
```

## Module responsibilities

- `cli` owns the stable user-facing command and option schema. It performs no
  hardware or filesystem work.
- `app` resolves precedence (`CLI > rosco.toml > defaults`) and composes the
  use cases. In particular, `run` keeps one serial port open across upload and
  monitoring so there is no reconnect race.
- `build` runs the configured builder either in the board's toolchain image or
  on this computer, and verifies the declared artifact. Docker mounts the
  project below `/workspace` under its own directory name. The module knows
  which extra steps a board's build takes - the m68k libraries, the basename
  its `software.mk` wants - but not Makefile syntax or compiler details.
- `toolchain` knows which programs a board's host build calls and what
  installs them on this system. `build` refuses a host build without them and
  `doctor` reports on them.
- `serial` owns host-specific serial discovery/opening behind `serialport`'s
  cross-platform trait. All connections are 8-N-1 with no flow control so
  binary upload remains 8-bit clean.
- `kermit` owns packet encoding, negotiation, checksums, retries, and transfer
  progress for rosco_m68k. It has no CLI or UI dependency.
- `ihex` owns the rosco_6502 equivalent: Intel hex records, the EWozMon
  conversation that carries them, and the run command that follows. It is
  written over `Read + Write` rather than a serial port, so it is exercised
  against a fake monitor in unit tests and against the real firmware in the
  emulator.
- `init` owns the project templates, what `init` writes into `rosco.toml`, and
  the memory of the answers it was given last time; `prompt` owns the questions
  asked when there is a terminal to ask them in. Neither knows anything about
  building or uploading. The remembered answers are a convenience and nothing
  more: no command reads them to decide anything, so a file that is missing or
  stale costs only the offer to reuse it.
- `emulator` starts the machine and attaches its console, from either a build
  of the emulator on this computer or its image. A firmware project is handed
  over here as a ROM set rather than as something to load, which is the one
  place the two project types take different paths through the emulator. The two differ only in what
  the paths look like to whatever runs the emulator, so both go through one
  argument builder and one layout of what it was handed.
- `config` validates `rosco.toml` and rejects unknown keys so typos do not fail
  silently. The board is resolved first, and what it implies - toolchain image,
  emulated machine - is layered underneath both settings files, so a project or
  a user can still override it and `config list` can still say where each value
  came from.

## Core flow

```text
rosco run
  -> load configuration, board first
  -> a firmware project stops here unless it is going to the emulator
  -> Docker or the host toolchain, checked before anything starts
  -> execute builder (Docker mounts the project at /workspace/<name>)
  -> verify the .bin (or, for a firmware project, .rom) artifact
  -> firmware: lay it out as a ROM set and boot the machine on it
  -> resolve/open UART
  -> rosco_m68k: Kermit S -> F -> D... -> Z -> B
     rosco_6502: L -> :records... -> :00000001FF -> <addr>R
  -> interactive UART session (stdin/stdout with terminal raw mode)
```

## Portability model

There are two different architectures involved:

1. The `rosco` host binary is compiled natively for Linux/Windows and
   x86_64/AArch64. Platform details stay inside dependencies and host adapters.
2. The user's bare-metal application is compiled for Motorola 68k or 65C02,
   whichever board the project names, either in that board's Docker image (the
   reproducible path, and the default) or by compilers on PATH. The CLI never
   assumes host and target architectures are the same.

Native CI runners cover the four promised host combinations. The serial crate
is built without its optional Linux `libudev` feature, avoiding a dynamic
system-library dependency and keeping distribution simpler.

## Planned extension points

- A further board adds a `Board` variant, its defaults, a template pair, and a
  tool list; the use cases above it are already written in terms of the board
  rather than of one machine.
- A toolchain service can select or pull Docker toolchains, then feed the
  resulting image into the existing build adapter.
- Additional upload protocols can implement the upload use case beside Kermit
  and Intel hex.

These should be added when their user-facing behavior is defined; the current
boundaries are intentionally small enough to evolve without a plugin framework.
