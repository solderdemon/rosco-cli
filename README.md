# rosco CLI

`rosco` is a cross-platform development CLI for
[rosco_m68k](https://github.com/rosco-m68k/rosco_m68k). It provides one workflow
for compiling an application, uploading its binary through UART with Kermit,
and displaying the program's UART output.

The current milestone supports Linux and Windows hosts on x64 and ARM64. The
program being built still targets Motorola 68k through the rosco_m68k
cross-toolchain.

## Install from this checkout

Install a current Rust toolchain, then run:

```sh
cargo install --path .
rosco --help
```

Cargo installs a binary named `rosco`. Make sure Cargo's binary directory
(`$HOME/.cargo/bin` on Linux or `%USERPROFILE%\.cargo\bin` on Windows) is on
`PATH`.

For building rosco_m68k applications, the configured builder and the upstream
cross-toolchain must also be on `PATH`. Defaults follow upstream conventions:
GNU Make and `m68k-elf-rosco-gcc`.

## Typical workflow

Run these commands from an application that includes rosco_m68k's
`code/software/software.mk`:

```sh
rosco doctor
rosco ports                 # show USB-UART candidates
rosco ports --all           # include built-in/non-USB serial ports
rosco build
rosco upload --port /dev/ttyUSB0
rosco monitor --port /dev/ttyUSB0
```

The combined development loop is:

```sh
rosco run --port /dev/ttyUSB0
```

On Windows, use a port such as `COM3`. UART defaults to 38400 baud, 8 data bits,
no parity, one stop bit, and no flow control. `rosco monitor` is currently an
output monitor; interactive terminal input is planned as a separate capability.

Use `-C` to work with another project directory:

```sh
rosco -C examples/hello run --port COM3
```

## Configuration

Configuration is optional. Without `rosco.toml`, the CLI runs `make all` and
expects `<project-directory>.bin`, matching rosco_m68k's `software.mk`.

Copy [rosco.example.toml](rosco.example.toml) to `rosco.toml` when the project
needs a custom artifact, builder, working directory, environment, or serial
settings. CLI `--port` and `--baud` values override configuration.

On Windows, a project can select another Make executable without changing CLI
code:

```toml
[build]
program = "mingw32-make"
args = ["all"]
clean_args = ["clean"]
```

## Commands

- `rosco build [--clean]` — run the configured build and validate the artifact.
- `rosco upload [FILE]` — transfer a binary using the built-in Kermit sender.
- `rosco monitor` — stream raw UART output to stdout until Ctrl-C.
- `rosco run [--clean]` — build, upload, and monitor through one open port.
- `rosco ports [--all]` — list USB-UART candidates and their USB metadata;
  `--all` also shows built-in and non-USB serial ports.
- `rosco doctor` — check the builder, C cross-toolchain, and UART discovery.

See [Architecture](docs/architecture.md) for module boundaries and extension
points.

When `--port` is omitted, `upload`, `monitor`, and `run` automatically select a
single USB-UART device even if Linux reports many built-in `/dev/ttyS*` ports.
USB identifiers describe the adapter rather than the computer connected to its
UART pins, so multiple USB-UART devices still require `--port` or a configured
`serial.port`.

## Development

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --all-targets -- -D warnings
```

CI builds and tests native binaries on Linux x64, Linux ARM64, Windows x64, and
Windows ARM64.
