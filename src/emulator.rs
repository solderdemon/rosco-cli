//! Runs a program under the rosco emulator instead of on real hardware.
//!
//! The emulator's console is an emulated UART, and MAME can put that UART on a
//! TCP socket. MAME tries to connect before it falls back to listening, so we
//! listen first and let it come to us; that way nothing the firmware prints
//! during boot is lost to a race.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::{Board, Config, EmulatorSource};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// A container has an image to unpack and a runtime to start before the
/// emulator inside it gets as far as opening a socket.
const DOCKER_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct EmulatorOptions {
    /// Where the emulator itself comes from.
    pub runner: Runner,
    /// Machine to run, for example `rosco_m68k_010`.
    pub machine: String,
    /// Binary to load into memory and run once the firmware is up.
    pub program_binary: Option<PathBuf>,
    /// Firmware to run the machine on, instead of the one the emulator
    /// ships. With this the program under test is the whole machine.
    pub firmware: Option<Firmware>,
    /// Directory holding the firmware ROM sets.
    pub rom_path: Option<PathBuf>,
    /// Image to attach as the SPI SD card.
    pub sd_card: Option<PathBuf>,
    /// Extra arguments passed straight through.
    pub extra_args: Vec<String>,
}

/// A firmware image and the name the emulator expects to find it under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Firmware {
    pub image: PathBuf,
    /// The board's ROM file name, for example `rosco_6502.rom`.
    pub rom_file: String,
}

/// What actually runs the machine: a build of the emulator on this computer,
/// or the emulator image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Runner {
    Program(PathBuf),
    Image {
        name: String,
        platform: Option<String>,
    },
}

impl std::fmt::Display for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Program(path) => write!(f, "{}", path.display()),
            Self::Image { name, .. } => write!(f, "{name} (docker)"),
        }
    }
}

/// Which CPU a machine has, and therefore what code it can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    M68k,
    Mos6502,
}

impl std::fmt::Display for Family {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::M68k => "68k",
            Self::Mos6502 => "6502",
        })
    }
}

/// Works out the CPU family from a machine name.
///
/// The emulator names its machines after the boards, so the prefix is enough;
/// an unknown name gives `None` rather than a guess.
pub fn family(machine: &str) -> Option<Family> {
    if machine.starts_with("rosco_m68k") {
        Some(Family::M68k)
    } else if machine.starts_with("rosco_6502") {
        Some(Family::Mos6502)
    } else {
        None
    }
}

/// The CPU a board has, and therefore what its toolchain produces.
pub fn board_family(board: Board) -> Family {
    match board {
        Board::RoscoM68k => Family::M68k,
        Board::Rosco6502 => Family::Mos6502,
    }
}

/// A running emulator with its console attached.
pub struct EmulatorSession {
    child: Child,
    console: TcpStream,
    /// Directory on this computer that goes away with the session.
    scratch: Option<PathBuf>,
    /// Container to stop, when the emulator is running in one.
    container: Option<String>,
}

impl EmulatorSession {
    pub fn console(&self) -> Result<TcpStream> {
        self.console
            .try_clone()
            .context("could not clone the emulator console connection")
    }
}

impl Drop for EmulatorSession {
    fn drop(&mut self) {
        // Killing the `docker run` client on its own leaves the container it
        // started behind, so the container goes first.
        if let Some(container) = &self.container {
            remove_container(container);
        }
        // The emulator has no reason to outlive the session.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(scratch) = &self.scratch {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}

pub fn start(options: &EmulatorOptions) -> Result<EmulatorSession> {
    match &options.runner {
        Runner::Program(program) => start_here(options, program),
        Runner::Image { name, platform } => start_in_docker(options, name, platform.as_deref()),
    }
}

/// Runs a build of the emulator on this computer.
fn start_here(options: &EmulatorOptions, program: &Path) -> Result<EmulatorSession> {
    let console = Console::listen(Ipv4Addr::LOCALHOST)?;

    // MAME writes cfg, nvram, snapshots and CHD diffs relative to the working
    // directory, which here is the user's project. Send all of that to a
    // scratch directory that goes away with the session instead.
    let scratch = scratch_directory(console.port);
    std::fs::create_dir_all(&scratch)
        .with_context(|| format!("could not create {}", scratch.display()))?;

    // MAME resolves its ROM path against the working directory, which is the
    // user's project, not the emulator tree. Point it at the firmware
    // explicitly so `rosco run --emulator` works from anywhere.
    let own_firmware = match &options.firmware {
        Some(firmware) => Some(rom_set(&scratch, &options.machine, firmware)?),
        None => None,
    };
    let rom_path = own_firmware
        .clone()
        .or_else(|| options.rom_path.clone())
        .or_else(|| default_rom_path(program));

    let layout = Layout {
        // Image paths have to be absolute for the same reason as the ROMs.
        program_binary: options.program_binary.as_deref().map(absolute_arg),
        sd_card: options.sd_card.as_deref().map(absolute_arg),
        rom_path: rom_path.as_deref().map(absolute_arg),
        directories: scratch.clone().into_os_string(),
        console: format!("127.0.0.1:{}", console.port),
    };

    let mut command = Command::new(program);
    // `-video none` only swaps in a renderer that draws nothing - the OSD
    // still creates an SDL window, which shows up as an empty black one - so
    // SDL is pointed at its dummy driver as well. That also means no display
    // has to be present at all.
    command.env("SDL_VIDEODRIVER", "dummy");
    command.args(arguments(options, &layout));
    quiet(&mut command);

    let child = command.spawn().with_context(|| {
        let _ = std::fs::remove_dir_all(&scratch);
        format!(
            "could not start the emulator {}; \
             point --emulator-path, emulator.program in {}, or {} at it, or run it from \
             the image with `rosco config set emulator.source docker`",
            program.display(),
            crate::config::CONFIG_FILE,
            PROGRAM_ENV,
        )
    })?;

    attach(
        child,
        console,
        CONNECT_TIMEOUT,
        Some(scratch),
        None,
        options.firmware.is_some(),
    )
}

/// Runs the emulator image, which carries the firmware ROM sets with it and
/// needs nothing installed on this computer.
fn start_in_docker(
    options: &EmulatorOptions,
    image: &str,
    platform: Option<&str>,
) -> Result<EmulatorSession> {
    crate::build::ensure_docker_available()?;
    ensure_image(image, platform)?;
    remove_abandoned_containers();

    // The container has a loopback of its own, so a console offered on ours
    // is not one it can reach. It is bound on every interface instead, and
    // the container dials back through the address Docker keeps for the host.
    let console = Console::listen(Ipv4Addr::UNSPECIFIED)?;
    let container = format!("{CONTAINER_PREFIX}{}-{}", std::process::id(), console.port);

    let mut mounts: Vec<OsString> = Vec::new();
    let layout = container_layout(options, console.port, &mut mounts)?;

    let mut command = Command::new("docker");
    command.args(["run", "--rm", "--name", &container]);
    if let Some(platform) = platform {
        command.args(["--platform", platform]);
    }
    command.args(["--add-host", &format!("{DOCKER_HOST_ALIAS}:host-gateway")]);
    command.args(["--env", "SDL_VIDEODRIVER=dummy"]);
    command.args(&mounts);
    // The image's entrypoint is the emulator itself, so everything after the
    // name goes straight to it.
    command.arg(image);
    command.args(arguments(options, &layout));
    quiet(&mut command);

    let child = command
        .spawn()
        .context("could not start docker to run the emulator image")?;

    attach(
        child,
        console,
        DOCKER_CONNECT_TIMEOUT,
        None,
        Some(container),
        options.firmware.is_some(),
    )
}

/// What the container is handed and where it finds it, which is every path
/// the emulator is given rewritten as a place inside the container.
fn container_layout(
    options: &EmulatorOptions,
    port: u16,
    mounts: &mut Vec<OsString>,
) -> Result<Layout> {
    let machine = &options.machine;
    Ok(Layout {
        program_binary: options
            .program_binary
            .as_deref()
            .map(|binary| mount(mounts, binary, CONTAINER_PROGRAM, ReadOnly))
            .transpose()?,
        sd_card: options
            .sd_card
            .as_deref()
            .map(|image| mount(mounts, image, CONTAINER_SD_CARD, Writable))
            .transpose()?,
        // A firmware the project built replaces the one the image ships, and
        // a single file is all the container needs: Docker makes the set
        // directory around it. Failing that, a ROM directory that was handed
        // in goes in front of the image's own, so a partial set still boots.
        rom_path: match (&options.firmware, options.rom_path.as_deref()) {
            (Some(firmware), _) => {
                mount_file(
                    mounts,
                    &firmware.image,
                    &format!("{CONTAINER_ROMS}/{}/{}", machine, firmware.rom_file),
                );
                Some(OsString::from(CONTAINER_ROMS))
            }
            (None, Some(roms)) => {
                mount_directory(mounts, roms, CONTAINER_ROMS);
                Some(OsString::from(format!("{CONTAINER_ROMS};{IMAGE_ROMS}")))
            }
            (None, None) => None,
        },
        // Everything MAME writes as it runs stays inside the container, which
        // is gone the moment the session ends.
        directories: OsString::from("/tmp"),
        console: format!("{DOCKER_HOST_ALIAS}:{port}"),
    })
}

/// The arguments the emulator itself takes, wherever it is running.
fn arguments(options: &EmulatorOptions, layout: &Layout) -> Vec<OsString> {
    // The console is the whole interface here, so the emulator runs with no
    // video or audio.
    let mut arguments: Vec<OsString> = vec![
        options.machine.clone().into(),
        "-skip_gameinfo".into(),
        "-video".into(),
        "none".into(),
        "-sound".into(),
        "none".into(),
    ];
    for option in [
        "-cfg_directory",
        "-nvram_directory",
        "-snapshot_directory",
        "-diff_directory",
        "-input_directory",
        "-state_directory",
        "-comment_directory",
    ] {
        arguments.push(option.into());
        arguments.push(layout.directories.clone());
    }
    arguments.extend([
        "-terminal".into(),
        "null_modem".into(),
        "-bitb".into(),
        OsString::from(format!("socket.{}", layout.console)),
    ]);
    for (option, value) in [
        ("-rompath", &layout.rom_path),
        ("-quik", &layout.program_binary),
        ("-hard1", &layout.sd_card),
    ] {
        if let Some(value) = value {
            arguments.push(option.into());
            arguments.push(value.clone());
        }
    }
    arguments.extend(options.extra_args.iter().map(OsString::from));
    arguments
}

/// Where the emulator finds everything, written the way whatever runs it sees
/// the filesystem: paths on this computer, or paths inside the container.
struct Layout {
    program_binary: Option<OsString>,
    sd_card: Option<OsString>,
    rom_path: Option<OsString>,
    directories: OsString,
    console: String,
}

/// Where the container's emulator finds what it was handed. `/work` is the
/// image's working directory, and `/opt/rosco/roms` the firmware it ships.
const CONTAINER_PROGRAM: &str = "/work/program";
const CONTAINER_SD_CARD: &str = "/work/sd";
const CONTAINER_ROMS: &str = "/work/roms";
const IMAGE_ROMS: &str = "/opt/rosco/roms";
/// The host, as a container started by Docker can reach it.
const DOCKER_HOST_ALIAS: &str = "host.docker.internal";

use Mount::{ReadOnly, Writable};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mount {
    ReadOnly,
    Writable,
}

/// Hands one file to the container, keeping its name: the emulator decides
/// what a file is by its extension, so a quickload image has to still be
/// called `.bin`.
fn mount(
    mounts: &mut Vec<OsString>,
    path: &Path,
    directory: &str,
    access: Mount,
) -> Result<OsString> {
    let source = absolute(path);
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| {
            format!(
                "{} has no name the container could be given it under",
                path.display()
            )
        })?;
    let target = format!("{directory}/{name}");
    mounts.push("--volume".into());
    mounts.push(OsString::from(format!(
        "{}:{target}{}",
        crate::build::docker_volume_source(&source),
        if access == ReadOnly { ":ro" } else { "" }
    )));
    Ok(target.into())
}

fn mount_directory(mounts: &mut Vec<OsString>, path: &Path, target: &str) {
    mounts.push("--volume".into());
    mounts.push(OsString::from(format!(
        "{}:{target}:ro",
        crate::build::docker_volume_source(&absolute(path))
    )));
}

/// Hands one file to the container at exactly `target`, creating whatever
/// directories that needs. Used for a firmware, which the emulator will only
/// look for under a name and a directory of its choosing.
fn mount_file(mounts: &mut Vec<OsString>, path: &Path, target: &str) {
    mounts.push("--volume".into());
    mounts.push(OsString::from(format!(
        "{}:{target}:ro",
        crate::build::docker_volume_source(&absolute(path))
    )));
}

/// Lays out a ROM set the emulator will accept: a directory named after the
/// machine, holding the board's firmware under the name it looks for.
fn rom_set(scratch: &Path, machine: &str, firmware: &Firmware) -> Result<PathBuf> {
    if !firmware.image.is_file() {
        bail!(
            "firmware image does not exist: {}",
            firmware.image.display()
        );
    }

    let roms = scratch.join("roms");
    let set = roms.join(machine);
    std::fs::create_dir_all(&set).with_context(|| format!("could not create {}", set.display()))?;
    let target = set.join(&firmware.rom_file);
    std::fs::copy(&firmware.image, &target).with_context(|| {
        format!(
            "could not copy {} to {}",
            firmware.image.display(),
            target.display()
        )
    })?;
    Ok(roms)
}

/// Pulls the image the first time it is wanted, rather than leaving the
/// download to happen silently while the console waits for a connection.
fn ensure_image(image: &str, platform: Option<&str>) -> Result<()> {
    if crate::build::docker_image_available(image) {
        return Ok(());
    }

    eprintln!("Pulling {image}");
    let mut command = Command::new("docker");
    command.arg("pull");
    if let Some(platform) = platform {
        command.args(["--platform", platform]);
    }
    command.arg(image);
    let status = command
        .status()
        .context("could not run docker to pull the emulator image")?;
    if !status.success() {
        bail!("could not pull {image}; docker exited with {status}");
    }
    Ok(())
}

/// The name every container this CLI starts is given, which is also how the
/// abandoned ones are recognised later.
const CONTAINER_PREFIX: &str = "rosco-cli-";

/// Stops containers left behind by a session that was killed outright rather
/// than allowed to end: an emulator nobody is talking to still burns a core.
///
/// The name carries the process that started it, so a name whose process is
/// gone belongs to nobody.
fn remove_abandoned_containers() {
    let listed = Command::new("docker")
        .args([
            "ps",
            "--filter",
            &format!("name={CONTAINER_PREFIX}"),
            "--format",
            "{{.Names}}",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(listed) = listed else { return };

    for name in String::from_utf8_lossy(&listed.stdout).lines() {
        let owner = name
            .strip_prefix(CONTAINER_PREFIX)
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok());
        if owner.is_some_and(|owner| !process_exists(owner)) {
            remove_container(name);
        }
    }
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    // Signal 0 asks the question without sending anything.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Nothing is swept where the question cannot be asked cheaply; a container
/// still running is the safer mistake.
#[cfg(not(unix))]
fn process_exists(_pid: u32) -> bool {
    true
}

fn remove_container(container: &str) {
    let _ = Command::new("docker")
        .args(["rm", "--force", container])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn quiet(command: &mut Command) {
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());
}

/// Waits for the emulator to dial the console back, and cleans up after it if
/// it never does.
fn attach(
    mut child: Child,
    console: Console,
    timeout: Duration,
    scratch: Option<PathBuf>,
    container: Option<String>,
    own_firmware: bool,
) -> Result<EmulatorSession> {
    if let Some(stderr) = child.stderr.take() {
        relay_diagnostics(stderr, own_firmware);
    }

    let stream = match accept_console(&console.listener, &mut child, timeout) {
        Ok(stream) => stream,
        Err(error) => {
            if let Some(container) = &container {
                remove_container(container);
            }
            let _ = child.kill();
            let _ = child.wait();
            if let Some(scratch) = &scratch {
                let _ = std::fs::remove_dir_all(scratch);
            }
            return Err(error);
        }
    };

    Ok(EmulatorSession {
        child,
        console: stream,
        scratch,
        container,
    })
}

/// The port the emulator's console connects back to.
///
/// MAME tries to connect before it falls back to listening, so we listen
/// first and let it come to us; that way nothing the firmware prints during
/// boot is lost to a race.
struct Console {
    listener: TcpListener,
    port: u16,
}

impl Console {
    fn listen(address: Ipv4Addr) -> Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(address, 0))
            .context("could not open a local port for the emulator console")?;
        let port = listener
            .local_addr()
            .context("could not determine the emulator console port")?
            .port();
        listener
            .set_nonblocking(true)
            .context("could not configure the emulator console listener")?;
        Ok(Self { listener, port })
    }
}

/// Forwards the emulator's diagnostics, minus the ones this run has already
/// accounted for.
fn relay_diagnostics(stderr: std::process::ChildStderr, own_firmware: bool) {
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
            if !expected(&line, own_firmware) {
                eprintln!("{line}");
            }
        }
    });
}

/// Whether a line the emulator printed is one this run asked for.
///
/// Its warning about running without video the console session makes moot.
/// The rest are the emulator noticing that the firmware is not the one it
/// shipped with - which is the entire point of a firmware project, and would
/// otherwise be four alarming lines on every single run.
fn expected(line: &str, own_firmware: bool) -> bool {
    const VIDEO: &str = "-video none doesn't make much sense";
    const SUBSTITUTED: &[&str] = &[
        "WRONG LENGTH",
        "WRONG CHECKSUMS",
        "EXPECTED: CRC",
        "FOUND: CRC",
        "WARNING: the machine might not run correctly.",
    ];

    if line.contains(VIDEO) {
        return true;
    }
    own_firmware && SUBSTITUTED.iter().any(|noise| line.contains(noise))
}

/// Per-session directory for the files the emulator writes as it runs.
fn scratch_directory(port: u16) -> PathBuf {
    std::env::temp_dir().join(format!("rosco-cli-{}-{port}", std::process::id()))
}

fn accept_console(
    listener: &TcpListener,
    child: &mut Child,
    timeout: Duration,
) -> Result<TcpStream> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nodelay(true)
                    .context("could not configure the emulator console connection")?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error).context("emulator console connection failed"),
        }

        if let Some(status) = child
            .try_wait()
            .context("could not check on the emulator process")?
        {
            bail!("the emulator exited before connecting its console ({status})");
        }

        if Instant::now() >= deadline {
            bail!(
                "the emulator did not connect its console within {} seconds",
                timeout.as_secs()
            );
        }

        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Runs the emulator to completion without a console, collecting what the
/// machine printed. Used for scripted checks rather than interactive work.
pub fn capture(options: &EmulatorOptions, seconds: u32) -> Result<String> {
    let mut options = options.clone();
    options.extra_args.extend([
        "-seconds_to_run".into(),
        seconds.to_string(),
        "-nothrottle".into(),
    ]);

    let session = start(&options)?;
    let mut console = session.console()?;
    console
        .set_read_timeout(Some(Duration::from_secs(seconds as u64 + 30)))
        .context("could not set a read timeout on the emulator console")?;

    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match console.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => return Err(error).context("emulator console read failed"),
        }
    }

    Ok(String::from_utf8_lossy(&output).into_owned())
}

/// Works out what runs the emulator: the image, or a binary on this computer.
///
/// A path given on the command line is a build on this computer whatever the
/// settings say, since that is the only thing it could mean.
pub fn resolve_runner(config: &Config, from_cli: Option<&Path>) -> Result<Runner> {
    if from_cli.is_none() && config.emulator.source == EmulatorSource::Docker {
        return Ok(Runner::Image {
            name: config.emulator.docker.image.clone(),
            platform: config.emulator.docker.platform.clone(),
        });
    }
    Ok(Runner::Program(resolve_program(config, from_cli)?))
}

/// Works out which emulator binary to use.
///
/// Command line wins, then `rosco.toml`, then the environment, and finally a
/// bare `rosco-emulator` looked up on `PATH`.
pub fn resolve_program(config: &Config, from_cli: Option<&Path>) -> Result<PathBuf> {
    let from_env = std::env::var_os(PROGRAM_ENV).map(PathBuf::from);
    resolve_program_with(config, from_cli, from_env)
}

/// The environment is passed in so the precedence rules can be tested without
/// depending on whatever the caller happens to have set.
fn resolve_program_with(
    config: &Config,
    from_cli: Option<&Path>,
    from_env: Option<PathBuf>,
) -> Result<PathBuf> {
    let program = from_cli
        .map(Path::to_path_buf)
        .or_else(|| config.emulator.program.clone())
        .or(from_env)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PROGRAM));

    // Built in its own source tree the emulator binary is called `rosco`,
    // which is this program's name too, so guard against re-running ourselves.
    if let Ok(current) = std::env::current_exe() {
        if same_file(&program, &current) {
            bail!(
                "the configured emulator ({}) is this CLI itself; \
                 point emulator.program or ROSCO_EMULATOR at the emulator binary",
                program.display()
            );
        }
    }

    Ok(program)
}

pub const DEFAULT_PROGRAM: &str = "rosco-emulator";
pub const PROGRAM_ENV: &str = "ROSCO_EMULATOR";

/// The emulator keeps its firmware in `roms/` next to the binary, so use that
/// when the caller has not said otherwise.
fn default_rom_path(program: &Path) -> Option<PathBuf> {
    let roms = program.canonicalize().ok()?.parent()?.join("roms");
    roms.is_dir().then_some(roms)
}

fn absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn absolute_arg(path: &Path) -> OsString {
    absolute(path).into_os_string()
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Removes ANSI escape sequences from console output.
///
/// The rosco firmware colourises what it prints, so a literal `Memory checks:
/// passed` never appears in the raw stream - there is a colour change sitting
/// in the middle of it. Anything matching against the console has to look at
/// the text with the escapes taken out.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();

    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }

        match chars.next() {
            // CSI: parameters and intermediates, then a final byte.
            Some('[') => {
                for ch in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        break;
                    }
                }
            }
            // OSC: runs until BEL or ST.
            Some(']') => {
                while let Some(ch) = chars.next() {
                    if ch == '\u{7}' {
                        break;
                    }
                    if ch == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            // Any other two-character escape.
            Some(_) | None => {}
        }
    }

    out
}

/// Writes `input` to the machine, used by scripted sessions.
pub fn send(console: &mut TcpStream, input: &[u8]) -> Result<()> {
    console
        .write_all(input)
        .context("could not write to the emulator console")?;
    console
        .flush()
        .context("could not flush the emulator console")
}

pub fn console_address(stream: &TcpStream) -> Option<SocketAddr> {
    stream.peer_addr().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(machine: &str) -> EmulatorOptions {
        EmulatorOptions {
            runner: Runner::Image {
                name: "solderdemon/rosco-emulator:latest".into(),
                platform: None,
            },
            machine: machine.into(),
            program_binary: None,
            firmware: None,
            rom_path: None,
            sd_card: None,
            extra_args: Vec::new(),
        }
    }

    fn value_after(arguments: &[OsString], option: &str) -> Option<String> {
        arguments
            .windows(2)
            .find(|pair| pair[0] == option)
            .map(|pair| pair[1].to_string_lossy().into_owned())
    }

    #[test]
    fn the_console_is_where_the_emulator_is_told_to_dial() {
        let arguments = arguments(
            &options("rosco_6502"),
            &Layout {
                program_binary: None,
                sd_card: None,
                rom_path: None,
                directories: "/tmp".into(),
                console: "127.0.0.1:4321".into(),
            },
        );

        assert_eq!(arguments[0], "rosco_6502");
        assert_eq!(
            value_after(&arguments, "-bitb").as_deref(),
            Some("socket.127.0.0.1:4321")
        );
        // Nothing was handed to it, so it is not told to load anything.
        assert!(value_after(&arguments, "-quik").is_none());
        assert!(value_after(&arguments, "-rompath").is_none());
    }

    #[test]
    fn a_container_is_given_the_binary_under_the_name_it_had() {
        let mut options = options("rosco_m68k_010");
        options.program_binary = Some("/home/me/hello/hello.bin".into());

        let mut mounts = Vec::new();
        let layout = container_layout(&options, 5000, &mut mounts).unwrap();

        // The emulator decides what a file is by its extension, so the name
        // has to survive the crossing.
        assert_eq!(
            mounts,
            [
                "--volume",
                "/home/me/hello/hello.bin:/work/program/hello.bin:ro"
            ]
        );
        let arguments = arguments(&options, &layout);
        assert_eq!(
            value_after(&arguments, "-quik").as_deref(),
            Some("/work/program/hello.bin")
        );
        assert_eq!(
            value_after(&arguments, "-bitb").as_deref(),
            Some("socket.host.docker.internal:5000")
        );
    }

    #[test]
    fn an_sd_card_is_writable_and_the_roms_are_not() {
        let mut options = options("rosco_m68k_010");
        options.sd_card = Some("/home/me/sd.img".into());
        options.rom_path = Some("/home/me/roms".into());

        let mut mounts = Vec::new();
        let layout = container_layout(&options, 5000, &mut mounts).unwrap();

        assert!(
            mounts.contains(&OsString::from("/home/me/sd.img:/work/sd/sd.img")),
            "{mounts:?}"
        );
        assert!(
            mounts.contains(&OsString::from("/home/me/roms:/work/roms:ro")),
            "{mounts:?}"
        );
        // The image's own firmware stays behind what was handed in, so a
        // directory holding one ROM set still boots the others.
        let arguments = arguments(&options, &layout);
        assert_eq!(
            value_after(&arguments, "-rompath").as_deref(),
            Some("/work/roms;/opt/rosco/roms")
        );
    }

    #[test]
    fn a_firmware_reaches_the_container_where_the_emulator_looks_for_it() {
        let mut options = options("rosco_m68k_010");
        options.firmware = Some(Firmware {
            image: "/home/me/hello/hello.rom".into(),
            rom_file: "rosco_m68k.rom".into(),
        });

        let mut mounts = Vec::new();
        let layout = container_layout(&options, 5000, &mut mounts).unwrap();

        // Docker makes the set directory around the one file it is given.
        assert_eq!(
            mounts,
            [
                "--volume",
                "/home/me/hello/hello.rom:/work/roms/rosco_m68k_010/rosco_m68k.rom:ro"
            ]
        );
        let arguments = arguments(&options, &layout);
        assert_eq!(
            value_after(&arguments, "-rompath").as_deref(),
            Some("/work/roms")
        );
    }

    #[test]
    fn a_firmware_is_laid_out_as_the_rom_set_the_emulator_expects() {
        let scratch = tempfile::tempdir().unwrap();
        let image = scratch.path().join("hello.rom");
        std::fs::write(&image, b"\x00\x10\x00\x00").unwrap();

        let roms = rom_set(
            scratch.path(),
            "rosco_6502",
            &Firmware {
                image,
                rom_file: "rosco_6502.rom".into(),
            },
        )
        .unwrap();

        assert_eq!(roms, scratch.path().join("roms"));
        assert!(roms.join("rosco_6502").join("rosco_6502.rom").is_file());
    }

    #[test]
    fn an_image_that_was_never_built_says_so_rather_than_booting_the_stock_firmware() {
        let scratch = tempfile::tempdir().unwrap();
        let error = rom_set(
            scratch.path(),
            "rosco_6502",
            &Firmware {
                image: "/nowhere/hello.rom".into(),
                rom_file: "rosco_6502.rom".into(),
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("does not exist"), "{error}");
    }

    #[test]
    fn the_emulator_noticing_a_substituted_firmware_is_not_news() {
        let complaint = "rosco_m68k.rom WRONG LENGTH (expected: 000da000 found: 000000e6)";
        assert!(expected(complaint, true));
        // In a project that did not replace the firmware, the same line means
        // the emulator's own ROM set is broken, and that is worth saying.
        assert!(!expected(complaint, false));
    }

    #[test]
    fn the_saved_source_decides_what_runs_the_machine() {
        let mut config = Config::default();
        assert!(matches!(
            resolve_runner(&config, None).unwrap(),
            Runner::Program(_)
        ));

        config.emulator.source = EmulatorSource::Docker;
        assert_eq!(
            resolve_runner(&config, None).unwrap(),
            Runner::Image {
                name: "solderdemon/rosco-emulator:latest".into(),
                platform: None,
            }
        );
    }

    #[test]
    fn a_path_on_the_command_line_is_a_build_on_this_computer() {
        let mut config = Config::default();
        config.emulator.source = EmulatorSource::Docker;

        let runner = resolve_runner(&config, Some(Path::new("/from/cli"))).unwrap();

        assert_eq!(runner, Runner::Program("/from/cli".into()));
    }

    #[test]
    fn machine_names_map_to_cpu_families() {
        assert_eq!(family("rosco_m68k_000"), Some(Family::M68k));
        assert_eq!(family("rosco_m68k_030"), Some(Family::M68k));
        assert_eq!(family("rosco_6502"), Some(Family::Mos6502));
        assert_eq!(family("something_else"), None);
    }

    #[test]
    fn strips_the_colours_the_firmware_prints() {
        let raw = "\u{1b}[0;37mMemory checks: \u{1b}[1;32mpassed\u{1b}[0m\r\n";
        assert_eq!(strip_ansi(raw), "Memory checks: passed\r\n");
    }

    #[test]
    fn leaves_plain_text_alone() {
        assert_eq!(strip_ansi("QUICKLOAD OK (6502)"), "QUICKLOAD OK (6502)");
    }

    #[test]
    fn handles_a_truncated_escape_at_the_end() {
        assert_eq!(strip_ansi("done\u{1b}["), "done");
        assert_eq!(strip_ansi("done\u{1b}"), "done");
    }

    fn env(path: &str) -> Option<PathBuf> {
        Some(PathBuf::from(path))
    }

    #[test]
    fn command_line_beats_config_and_environment() {
        let mut config = Config::default();
        config.emulator.program = Some(PathBuf::from("/from/config"));
        let resolved =
            resolve_program_with(&config, Some(Path::new("/from/cli")), env("/from/env")).unwrap();
        assert_eq!(resolved, PathBuf::from("/from/cli"));
    }

    #[test]
    fn config_beats_the_environment() {
        let mut config = Config::default();
        config.emulator.program = Some(PathBuf::from("/from/config"));
        let resolved = resolve_program_with(&config, None, env("/from/env")).unwrap();
        assert_eq!(resolved, PathBuf::from("/from/config"));
    }

    #[test]
    fn environment_is_used_when_nothing_else_says_otherwise() {
        let resolved = resolve_program_with(&Config::default(), None, env("/from/env")).unwrap();
        assert_eq!(resolved, PathBuf::from("/from/env"));
    }

    #[test]
    fn falls_back_to_a_name_looked_up_on_path() {
        let resolved = resolve_program_with(&Config::default(), None, None).unwrap();
        assert_eq!(resolved, PathBuf::from(DEFAULT_PROGRAM));
    }

    #[test]
    fn rom_path_is_taken_from_beside_the_emulator_binary() {
        let dir = std::env::temp_dir().join(format!("rosco-emu-test-{}", std::process::id()));
        let bin = dir.join("rosco");
        std::fs::create_dir_all(dir.join("roms")).unwrap();
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();

        assert_eq!(
            default_rom_path(&bin),
            Some(dir.canonicalize().unwrap().join("roms"))
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rom_path_is_absent_when_there_is_no_roms_directory() {
        assert_eq!(default_rom_path(Path::new("/nonexistent/rosco")), None);
    }

    #[test]
    fn refuses_to_run_this_cli_as_the_emulator() {
        let current = std::env::current_exe().unwrap();
        let error = resolve_program_with(&Config::default(), Some(&current), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("this CLI itself"), "{error}");
    }
}
