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

Create a project, then build and run it:

```sh
rosco init c hello
cd hello
rosco build
```

`init` creates the destination directory only when it does not already exist.
It copies the shared build files and libraries plus the selected `c` or `asm`
starter source. On the first build, `rosco` builds the bundled libraries; later
builds reuse `libs/build`.

For an existing project, use:

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
no parity, one stop bit, and no flow control. `rosco monitor` opens a bidirectional
interactive UART session with terminal raw mode (press `Ctrl-C` to exit).

Use `-C` to work with another project directory:

```sh
rosco -C examples/hello run --port COM3
```

## Settings

Anything you would otherwise pass on every run can be saved once:

```sh
rosco config set serial.port /dev/ttyUSB0   # then just `rosco run`
rosco config set serial.baud 9600
rosco config set defaults.target emulator   # no more --emulator
rosco config list                           # every setting and where it came from
```

There are two settings files:

- the per-user one, which applies to every project on this machine. It lives in
  `$XDG_CONFIG_HOME/rosco/config.toml` (`~/.config/rosco/config.toml` by
  default) on Linux and macOS, and `%APPDATA%\rosco\config.toml` on Windows.
  `ROSCO_CONFIG_HOME` overrides the directory. This is where the UART you have
  plugged in and the path to your emulator build belong.
- the project's `rosco.toml`, for what belongs to the application: its artifact,
  its builder, its machine. Write to it with `--local`.

The project file wins over the per-user one, key by key, and command-line
options win over both. Only write a setting into `rosco.toml` when the project
really does need its own value: one repeated from the defaults pins it there
and quietly overrides what you saved for yourself.

```sh
rosco config list           # the origin column says which file decided each value
```


```sh
rosco config set --local emulator.machine rosco_6502
rosco config unset serial.baud
rosco config get serial.port
rosco config path --local     # print the file a scope writes to
rosco config edit             # open it in $EDITOR
```

`rosco config list` is the full list of what can be saved:

| Setting | Meaning |
| --- | --- |
| `project.artifact` | Binary the build produces, when it is not `<directory>.bin` |
| `build.program`, `build.args`, `build.clean_args` | The build command and its arguments |
| `build.working_directory` | Where to run it, relative to the project |
| `build.environment.<NAME>` | A variable passed to the build process |
| `build.docker.image`, `build.docker.platform` | Toolchain image, and platform on ARM hosts |
| `serial.port`, `serial.baud`, `serial.read_timeout_ms` | UART device and line settings |
| `upload.max_retries`, `upload.packet_timeout_ms` | Kermit transfer behaviour |
| `emulator.program`, `emulator.machine`, `emulator.rom_path`, `emulator.sd_card`, `emulator.args` | Which emulator to run and how |
| `defaults.target` | `hardware` or `emulator` |

`config set` refuses a name that is not a real setting and a value of the wrong
type, so a typo is caught at the point you make it rather than on the next
build. Edits keep the rest of the file, comments included, as you wrote it.

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

- `rosco init <c|asm> <destination>` — create a new rosco_m68k starter project.
- `rosco build [--clean]` — run the configured build and validate the artifact.
- `rosco upload [FILE]` — transfer a binary using the built-in Kermit sender,
  or load it into the emulator with `--emulator`.
- `rosco monitor` — open an interactive UART session (press Ctrl-C to exit).
- `rosco run [--clean]` — build, upload, and open an interactive UART session.
- `rosco ports [--all]` — list USB-UART candidates and their USB metadata;
  `--all` also shows built-in and non-USB serial ports.
- `rosco config <list|get|set|unset|path|edit>` — read and write saved
  settings; `--local` targets the project's `rosco.toml` instead of the
  per-user file.
- `rosco doctor` — check Docker, local build tools, the emulator, and UART
  discovery.

## Running in the emulator

`--emulator` swaps the board on the other end of the UART for the
[rosco emulator](https://github.com/solderdemon/rosco-emulator), so a program
can be built and run with no hardware attached:

```sh
rosco run --emulator                       # build, load, and attach the console
rosco upload hello.bin --emulator          # run a binary that is already built
rosco monitor --emulator                   # just boot the firmware
rosco run --emulator --machine rosco_6502  # the 6502 board instead
```

With no board on the desk at all, save the choice instead of repeating the
flag; `--hardware` then overrides it for a single run:

```sh
rosco config set defaults.target emulator
rosco run
rosco run --hardware --port /dev/ttyUSB0
```

There is no Kermit transfer involved. The emulator loads the binary straight
into memory once the firmware has finished booting, which is both faster and
closer to what the firmware would have ended up with anyway.

The console is a real bidirectional session, exactly like the UART one: the
emulator connects its emulated UART back to a local socket that the CLI listens
on, so nothing the firmware prints during boot is missed.

The emulator always runs headless, with the console as its only interface; it
does not need a display to be present at all. For the emulator's own UI and
debugger, run its binary directly.

Other options: `--sd-card IMAGE` attaches an SD card and `--rom-path DIR` points
at a different firmware set.

### Which machine a project runs on

The machine is a property of the project, so it belongs in its `rosco.toml`:

```toml
[emulator]
machine = "rosco_m68k_010"
```

`--machine` overrides that for one run, and with neither the default is
`rosco_m68k_010`. Which of the four m68k machines you pick does not affect
whether a binary works - they all run the same 68000 code - only how fast the
CPU is and which instructions are available.

What does matter is the CPU family. `rosco run` builds the artifact itself,
with the 68k toolchain, so it knows the result cannot run on a 6502 and says so
instead of letting the machine crash into its monitor:

```
this project builds 68k code, but rosco_6502 is a 6502 machine;
set emulator.machine in rosco.toml or pass --machine
```

`rosco upload` takes a file it did not build and has no way to tell what is in
it, so it runs whatever you point it at.

### Finding the emulator

The CLI looks for the emulator in this order: `--emulator-path`, then
`emulator.program` from the settings files, then the `ROSCO_EMULATOR`
environment variable, and finally `rosco-emulator` on `PATH`. Since a build of
the emulator belongs to the machine rather than to one project, save it once
with `rosco config set emulator.program /path/to/rosco-emulator`.

Note that the emulator's own binary is called `rosco` when built in its source
tree, which is this CLI's name too. Either point at it explicitly or install it
under a different name; the CLI refuses to run itself if the two ever resolve to
the same file.

Firmware ROMs are taken from `roms/` next to the emulator binary unless
`--rom-path` or `emulator.rom_path` says otherwise.

See [Architecture](docs/architecture.md) for module boundaries and extension
points.

When `--port` is omitted, `upload`, `monitor`, and `run` automatically select a
single USB-UART device even if Linux reports many built-in `/dev/ttyS*` ports.
USB identifiers describe the adapter rather than the computer connected to its
UART pins, so multiple USB-UART devices still require `--port` or a saved
`serial.port`.

## Development

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --all-targets -- -D warnings
```

CI builds and tests native binaries on Linux x64, Linux ARM64, Windows x64, and
Windows ARM64.
