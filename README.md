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

`rosco build` and `rosco run` ask where to build: Docker (recommended) or the
local computer. Docker runs GNU Make in `roscopeco/rosco_m68k:latest`, which
contains the complete cross-toolchain. A local build checks for its toolchain
and, with explicit confirmation, installs the required Homebrew packages on
Linux and macOS.

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
When Docker is selected, the project directory is mounted below `/workspace`
with its original directory name. This preserves rosco_m68k's default artifact
name (`<project-directory>.bin`). Generated files stay on the host and, on
Unix, retain the project directory's ownership. If the configured Docker image
is absent, `rosco` pulls it before starting the build.

Copy [rosco.example.toml](rosco.example.toml) to `rosco.toml` when the project
needs a custom artifact, builder, working directory, Docker image, environment, or serial
settings. CLI `--port` and `--baud` values override configuration.

The builder command is used in both modes. Docker settings apply when Docker
is selected:

```toml
[build]
program = "make"
args = ["all"]
clean_args = ["clean"]

[build.docker]
image = "roscopeco/rosco_m68k:latest"
```

On ARM hosts, set `build.docker.platform = "linux/amd64"` when using the
published image. Docker will use emulation if necessary.

For a local build, `rosco` checks the configured builder plus
`m68k-elf-rosco-gcc`. If either is missing, it asks before installing Homebrew
and the rosco_m68k toolchain, Make, Git, Python, vasm, and srecord. Automatic
installation is intentionally not offered on Windows; select Docker there.

To use the image built from your local `rosco_m68k_docker` fork, build it with
its `make image` target and set `image = "rosco-m68k-toolchain:local"`.

## Commands

- `rosco build [--clean]` — run the configured build and validate the artifact.
- `rosco upload [FILE]` — transfer a binary using the built-in Kermit sender.
- `rosco monitor` — stream raw UART output to stdout until Ctrl-C.
- `rosco run [--clean]` — build, upload, and monitor through one open port.
- `rosco ports [--all]` — list USB-UART candidates and their USB metadata;
  `--all` also shows built-in and non-USB serial ports.
- `rosco doctor` — check Docker, local build tools, and UART discovery.

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
