# rosco CLI

`rosco` is a cross-platform development CLI for
[rosco_m68k](https://github.com/rosco-m68k/rosco_m68k) and
[rosco_6502](https://github.com/rosco-6502/rosco_6502). It provides one workflow
for compiling an application, uploading its binary through UART, and displaying
the program's output - on either board, and with or without the hardware.

The current milestone supports Linux and Windows hosts on x64 and ARM64. The
program being built targets Motorola 68k or 65C02, whichever board its project
names.

## Install from this checkout

Install a current Rust toolchain, then run:

```sh
cargo install --path .
rosco --help
```

Cargo installs a binary named `rosco`. Make sure Cargo's binary directory
(`$HOME/.cargo/bin` on Linux or `%USERPROFILE%\.cargo\bin` on Windows) is on
`PATH`.

## Typical workflow

`rosco init` asks what to create and then creates it:

```text
$ rosco init

Create a rosco project

+ Project name: hello
? Board
  > rosco_m68k  68010 board, C or assembly through the m68k toolchain
    rosco_6502  65C02 board, C or assembly through the cc65 tools
```

It asks for the name, the board, whether the project is a program or the
firmware, the language, where the compilers come from, whether `run` goes to
hardware or to the emulator, and where the emulator itself comes from. Every
answer has an option of its own, so a script never waits for one:

```sh
rosco init hello --board rosco_6502 --language asm --host --target emulator --yes
rosco init hello --type firmware                      # the machine, not a program
rosco init hello --emulator-docker                    # emulator from its image
rosco init hello --emulator-path ~/rosco-emulator/rosco
rosco init c hello          # the older form still works
```

The emulator question is the one that saves the most typing later. Answering
it writes the choice into the project, so no later command needs
`--emulator-path`:

```text
? Emulator
  > Docker           the emulator image; nothing to install
    This computer    a build of it, saved in the project so no flag is needed
    Decide each run  ROSCO_EMULATOR, or rosco-emulator on PATH
```

### Doing it again

`init` remembers the answers it was given, in `init.toml` beside the per-user
settings, and offers them back the next time rather than asking the same five
questions:

```text
? Setup
  > Previous setup  rosco_6502/asm, host build, emulator, docker emulator
    New setup       answer the questions again
```

Choosing to answer again starts each question on what was chosen last time, so
changing one thing is a few keystrokes rather than a whole conversation.
Deleting `init.toml` starts from the defaults again; nothing else reads it.

`init` creates the destination directory only when it does not already exist.
It copies the starter sources for the board and language, and writes what you
chose into the project's `rosco.toml`, so the commands that follow need no
flags:

```sh
cd hello
rosco build
```

For rosco_m68k, the first build also builds the bundled libraries; later builds
reuse `libs/build`. For rosco_6502, the project is a Makefile, two sources and
the linker configuration, and there is nothing to build first.

## Programs and firmware

Almost every project is a **program**: the firmware on the board brings the
hardware up, then loads the program into RAM and jumps to it. That is why a
`main()` can print a line without configuring a UART first, and why, after it
returns, the board is back at its monitor prompt.

The other kind is the **firmware** itself - what the machine starts in, with
nothing having run before it:

```sh
rosco init hello --type firmware --board rosco_6502
cd hello
rosco run
```

```text
Hello from my own firmware!
Type something; it comes back.
```

That is a whole machine. The CPU takes its reset vector out of the ROM, the
template sets up the stack, programs the DUART for 115200 baud and drives it a
byte at a time; there is no BIOS underneath it because the BIOS is what it
replaced. Both boards have a template: assembly, one file, and a Makefile that
produces a ROM image rather than a binary.

|  | program | firmware |
| --- | --- | --- |
| Built | `hello.bin` | `hello.rom` |
| Starts at | `$0800` / `0x40000`, after the firmware boots | the reset vector, from cold |
| Reaches the board | UART, with `rosco upload` | an EEPROM programmer |
| `rosco run` | loads it into a booted machine | boots the machine on it |

A firmware project runs in the emulator, which takes the image in place of the
ROM set it ships with. It cannot be sent to a board - there is no way to put a
ROM in a socket down a serial cable - so `run --hardware` says so instead of
failing later:

```
this project builds firmware, which a board reads from its ROM rather than
over the UART.
  Run it here with `rosco run --emulator`, or burn the image to an EEPROM
  and put it in the socket.
```

Because the image is not the firmware the emulator was built around, the
emulator notices the checksum is not the one it expected. That is the point of
the exercise rather than a problem, so those lines are not passed on.

## Boards

The board is a property of the project, and it decides the rest: which
toolchain compiles it, which machine the emulator runs, and how a binary gets
into the board over the UART.

| | rosco_m68k | rosco_6502 |
| --- | --- | --- |
| Toolchain | `m68k-elf-rosco-gcc`, vasm | the cc65 tools: `ca65`, `ld65`, `cl65` |
| Toolchain image | `solderdemon/rosco_m68k:latest` | `solderdemon/rosco_6502:latest` |
| Installed with | `brew tap rosco-m68k/toolchain` | `apt install cc65`, `brew install cc65` |
| Upload | Kermit, to the firmware's loader | Intel hex, through the EWozMon monitor |
| Emulated machine | `rosco_m68k_010` | `rosco_6502` |

`rosco init` writes the board into `rosco.toml`; an existing project can be
told which one it is with:

```sh
rosco config set --local project.board rosco_6502
```

### What a rosco_6502 project contains

```text
hello/
  Makefile           ca65 and ld65, or cl65 for a C project
  main.s / main.c    the program, which starts at $0800
  console.s          putchar, and print or the C library's write and read
  rosco_6502.cfg     ld65 memory layout: low RAM, one 32K bank, the C stack
  inc/defines.inc    the board's addresses and the firmware's BIOS calls
  rosco.toml         the board, and anything else init was told
```

`console.s` writes to the DUART directly rather than calling the firmware's
print routines. The firmware has them, and `inc/defines.inc` names them, but
which entry in the ROM jump table each one is has moved between firmware
revisions - so a program that calls them prints nothing at all on a board
whose ROM is older than the header it was built against. The XR68C681
registers do not move.

## Where the compilers come from

By default the build runs in the board's toolchain image, so nothing has to be
installed on this computer:

```sh
rosco build                 # in Docker
```

The cc65 tools that build rosco_6502 projects are a single package on every
desktop system, and a build that does not start a container is quicker. Either
answer the question `rosco init` asks, or say so afterwards:

```sh
rosco config set build.toolchain host
sudo apt install cc65       # or dnf, pacman, brew - `rosco doctor` says which
```

`--docker` and `--host` override the setting for one run:

```sh
rosco build --host
rosco run --docker
```

Whichever is chosen, `rosco` checks it before starting: a missing Docker daemon
or a missing compiler is one sentence naming what to do about it, rather than a
build that fails half-way through.

The rosco_m68k cross-compiler is not in any distribution's archive, so its
`host` toolchain means the Homebrew tap:

```sh
brew tap rosco-m68k/toolchain
brew install rosco-m68k-toolchain@13 vasm-all srecord
```

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

An emulator the CLI builds for you goes beside neither, in
`$XDG_DATA_HOME/rosco` (`~/.local/share/rosco` by default) or
`%LOCALAPPDATA%\rosco`, which `ROSCO_DATA_HOME` overrides.

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
| `project.board` | `rosco_m68k` or `rosco_6502` |
| `project.type` | `program`, or `firmware` for the ROM the machine starts in |
| `project.artifact` | Binary the build produces, when it is not `<directory>.bin` |
| `build.toolchain` | `docker` or `host` |
| `build.program`, `build.args`, `build.clean_args` | The build command and its arguments |
| `build.working_directory` | Where to run it, relative to the project |
| `build.environment.<NAME>` | A variable passed to the build process |
| `build.docker.image`, `build.docker.platform` | Toolchain image, and platform on ARM hosts |
| `serial.port`, `serial.baud`, `serial.read_timeout_ms` | UART device and line settings |
| `upload.max_retries`, `upload.packet_timeout_ms` | Transfer retries and how long to wait for an answer |
| `upload.load_address` | Where a rosco_6502 program loads and starts (`0x0800`) |
| `emulator.source` | `host` for a build of the emulator, `docker` for its image |
| `emulator.program`, `emulator.machine`, `emulator.rom_path`, `emulator.sd_card`, `emulator.args` | Which emulator to run and how |
| `emulator.docker.image`, `emulator.docker.platform` | Emulator image, and platform on ARM hosts |
| `defaults.target` | `hardware` or `emulator` |

`config set` refuses a name that is not a real setting and a value of the wrong
type, so a typo is caught at the point you make it rather than on the next
build. Edits keep the rest of the file, comments included, as you wrote it.

## Configuration

Configuration is optional. Without `rosco.toml`, the CLI builds a rosco_m68k
project with `make all` and expects `<project-directory>.bin`, matching
rosco_m68k's `software.mk`; the 6502 templates follow the same convention.
The project directory is mounted below `/workspace` under its own name, so a
Makefile that names its output after the directory it builds in produces the
same file in Docker as it does here. Generated files stay on the host and, on
Unix, retain the project directory's ownership. If the configured Docker image
is absent, `rosco` pulls it before starting the build.

Copy [rosco.example.toml](rosco.example.toml) to `rosco.toml` when the project
needs a custom artifact, builder, working directory, Docker image, environment, or serial
settings. CLI `--port` and `--baud` values override configuration.

The builder command is used both in the image and on this computer; the Docker
settings only apply to the first:

```toml
[project]
board = "rosco_6502"

[build]
program = "make"
args = ["all"]
clean_args = ["clean"]

[build.docker]
image = "solderdemon/rosco_6502:latest"
```

On ARM hosts, set `build.docker.platform = "linux/amd64"` when using the
published image. Docker will use emulation if necessary.

To use an image built from your local `rosco_m68k_docker` or
`rosco_6502_docker` fork, build it with its `make image` target and set
`image = "rosco-6502-toolchain:local"`.

## Commands

- `rosco init [NAME]` — create a new project, asking about anything the command
  line did not settle: `--board`, `--language`, `--docker`/`--host`,
  `--target`, and `--yes` to take the defaults for the rest.
- `rosco build [--clean] [--docker|--host]` — run the configured build and
  validate the artifact.
- `rosco upload [FILE]` — transfer a binary to the board, or load it into the
  emulator with `--emulator`.
- `rosco monitor` — open an interactive UART session (press Ctrl-C to exit).
- `rosco run [--clean]` — build, upload, and open an interactive UART session.
- `rosco ports [--all]` — list USB-UART candidates and their USB metadata;
  `--all` also shows built-in and non-USB serial ports.
- `rosco config <list|get|set|unset|path|edit>` — read and write saved
  settings; `--local` targets the project's `rosco.toml` instead of the
  per-user file.
- `rosco doctor` — check Docker, the board's toolchain, the emulator, and UART
  discovery.

## Uploading to a board

The two boards receive a program in the two ways their firmware knows.

On rosco_m68k, `upload` speaks Kermit to the loader in the firmware, which is
what `LOAD` on the firmware menu is waiting for.

On rosco_6502 there is no Kermit receiver. `upload` uses the monitor instead:
it types `L`, sends the binary as Intel hex records - one at a time, waiting
for the full stop the monitor answers each accepted line with, because that is
the only flow control the UART has - and then types the address to start it.
The board has to be at the `\` prompt when the transfer begins, which is where
a reset leaves it:

```sh
rosco upload hello.bin --port /dev/ttyUSB0
```

The records are made from the binary rather than from a `.hex` file beside it,
so the address a program loads at is a setting rather than something baked into
the build. It defaults to `$0800`, the bottom of user low RAM:

```sh
rosco config set --local upload.load_address 0x0800
```

## Running in the emulator

`--emulator` swaps the board on the other end of the UART for the
[rosco emulator](https://github.com/solderdemon/rosco-emulator), so a program
can be built and run with no hardware attached:

```sh
rosco run --emulator                       # build, load, and attach the console
rosco upload hello.bin --emulator          # run a binary that is already built
rosco monitor --emulator                   # just boot the firmware
rosco run --emulator --machine rosco_m68k_030   # another m68k machine
```

With no board on the desk at all, save the choice instead of repeating the
flag; `--hardware` then overrides it for a single run:

```sh
rosco config set defaults.target emulator
rosco run
rosco run --hardware --port /dev/ttyUSB0
```

No transfer is involved, on either board. The emulator loads the binary
straight into memory once the firmware has finished booting, which is both
faster and closer to what the firmware would have ended up with anyway.

The console is a real bidirectional session, exactly like the UART one: the
emulator connects its emulated UART back to a local socket that the CLI listens
on, so nothing the firmware prints during boot is missed.

The emulator always runs headless, with the console as its only interface; it
does not need a display to be present at all. For the emulator's own UI and
debugger, run its binary directly.

Other options: `--sd-card IMAGE` attaches an SD card and `--rom-path DIR` points
at a different firmware set.

### Which machine a project runs on

The board decides: a rosco_m68k project runs on `rosco_m68k_010` and a
rosco_6502 project on `rosco_6502`, without either being written down. A
project that wants another machine says so in its `rosco.toml`:

```toml
[emulator]
machine = "rosco_m68k_030"
```

`--machine` overrides that for one run. Which of the four m68k machines you
pick does not affect whether a binary works - they all run the same 68000 code -
only how fast the CPU is and which instructions are available.

What does matter is the CPU family. `rosco run` builds the artifact itself, so
it knows what the result cannot run on and says so instead of letting the
machine crash into its monitor:

```
this project builds 68k code, but rosco_6502 is a 6502 machine;
set emulator.machine in rosco.toml or pass --machine
```

`rosco upload` takes a file it did not build and has no way to tell what is in
it, so it runs whatever you point it at.

### Finding the emulator

`emulator.source` decides where the emulator comes from: `host`, a build of it
on this computer, or `docker`, its image. `rosco init` asks, and either answer
can be saved at any time:

```sh
rosco config set emulator.source docker             # for every project
rosco config set --local emulator.program ~/rosco-emulator/rosco
```

For a build on this computer the CLI looks in this order: `--emulator-path`,
then `emulator.program` from the settings files, then the `ROSCO_EMULATOR`
environment variable, and finally `rosco-emulator` on `PATH`. Since a build of
the emulator belongs to the machine rather than to one project, saving it
without `--local` shares it with every project.

Note that the emulator's own binary is called `rosco` when built in its source
tree, which is this CLI's name too. Either point at it explicitly or install it
under a different name; the CLI refuses to run itself if the two ever resolve to
the same file.

Firmware ROMs are taken from `roms/` next to the emulator binary unless
`--rom-path` or `emulator.rom_path` says otherwise.

### When there is no emulator

A run that needs the emulator and cannot find one asks what to do about it
instead of failing:

```text
The emulator is not ready

  rosco-emulator is not on PATH, and no emulator is saved in the settings.

? Emulator
  > Docker          run its image; nothing to install
    Build it here   clone the emulator and build it, which takes a while
    Existing build  point at an emulator already on this computer
    Cancel          stop, and print how to set one up later
```

**Docker** checks that Docker itself answers and then runs the image, which is
pulled by the run that follows. **Build it here** clones
[the emulator](https://github.com/solderdemon/rosco-emulator) into a directory
you choose and builds it, after saying what that costs: it is a MAME tree, so
the checkout is large and the build takes tens of minutes. It names anything
the build needs and is missing rather than failing halfway through, and its
releases carry prebuilt Linux binaries for anyone who would rather not build.
**Existing build** takes a path, and accepts the emulator's source tree as well
as the binary in it.

Whichever is chosen is saved, so the question is asked once: in `rosco.toml`
if this project pins the same setting, and in the user settings otherwise. The
same question comes up when `emulator.source = "docker"` but Docker is not
installed or is not running, and there the build on this computer is the answer
offered first.

Nothing is asked where there is no terminal to ask in - a script or a CI job
gets the same options as one message and a non-zero exit - and a wrong
`--emulator-path` is reported rather than turned into a conversation, since it
is an answer to this question already.

### The emulator in Docker

`emulator.source = "docker"` runs `solderdemon/rosco-emulator` instead, which
carries the emulator and the firmware ROM sets and needs nothing installed:

```sh
rosco config set emulator.source docker
rosco run                                  # pulls the image the first time
```

The image is pulled once, out loud, rather than silently on the first run that
needs it. `--emulator-path` still overrides it for a single run, since a path
can only mean a build on this computer.

What the emulator is handed is mounted into the container under the name it
had, because the emulator decides what a file is by its extension: the binary
read-only, an `--sd-card` image writable, and a `--rom-path` directory in front
of the firmware the image ships. The console reaches back out through
`host.docker.internal`, and the container is named after the session that
started it, so one left behind by a session that was killed outright is stopped
by the next run rather than left burning a core.

`emulator.docker.image` changes the image and `emulator.docker.platform` asks
for a particular one on an ARM host.

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

The tests in `tests/emulator.rs` run a real emulator when `ROSCO_EMULATOR`
points at one, and are skipped when it does not. They cover the firmware's
power-on self test and the whole 6502 upload conversation, from `L` to the
program printing for itself:

```sh
ROSCO_EMULATOR=/path/to/rosco-emulator cargo test
```

CI builds and tests native binaries on Linux x64, Linux ARM64, Windows x64, and
Windows ARM64.
