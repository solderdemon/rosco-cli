//! Runs a program under the rosco emulator instead of on real hardware.
//!
//! The emulator's console is an emulated UART, and MAME can put that UART on a
//! TCP socket. MAME tries to connect before it falls back to listening, so we
//! listen first and let it come to us; that way nothing the firmware prints
//! during boot is lost to a race.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::Config;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub struct EmulatorOptions {
    /// Emulator executable.
    pub program: PathBuf,
    /// Machine to run, for example `rosco_m68k_010`.
    pub machine: String,
    /// Binary to load into memory and run once the firmware is up.
    pub program_binary: Option<PathBuf>,
    /// Directory holding the firmware ROM sets.
    pub rom_path: Option<PathBuf>,
    /// Image to attach as the SPI SD card.
    pub sd_card: Option<PathBuf>,
    /// Extra arguments passed straight through.
    pub extra_args: Vec<String>,
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

/// A running emulator with its console attached.
pub struct EmulatorSession {
    child: Child,
    console: TcpStream,
    scratch: PathBuf,
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
        // The emulator has no reason to outlive the session.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

pub fn start(options: &EmulatorOptions) -> Result<EmulatorSession> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .context("could not open a local port for the emulator console")?;
    let port = listener
        .local_addr()
        .context("could not determine the emulator console port")?
        .port();
    listener
        .set_nonblocking(true)
        .context("could not configure the emulator console listener")?;

    // MAME writes cfg, nvram, snapshots and CHD diffs relative to the working
    // directory, which here is the user's project. Send all of that to a
    // scratch directory that goes away with the session instead.
    let scratch = scratch_directory(port);
    std::fs::create_dir_all(&scratch)
        .with_context(|| format!("could not create {}", scratch.display()))?;

    // The console is the whole interface here, so the emulator runs with no
    // video or audio. `-video none` only swaps in a renderer that draws
    // nothing - the OSD still creates an SDL window, which shows up as an
    // empty black one - so SDL is pointed at its dummy driver as well. That
    // also means no display has to be present at all.
    let mut command = Command::new(&options.program);
    command.env("SDL_VIDEODRIVER", "dummy");
    command.arg(&options.machine);
    command.args(["-skip_gameinfo", "-video", "none", "-sound", "none"]);
    for option in [
        "-cfg_directory",
        "-nvram_directory",
        "-snapshot_directory",
        "-diff_directory",
        "-input_directory",
        "-state_directory",
        "-comment_directory",
    ] {
        command.arg(option).arg(&scratch);
    }
    command.args([
        "-terminal",
        "null_modem",
        "-bitb",
        &format!("socket.127.0.0.1:{port}"),
    ]);
    // MAME resolves its ROM path against the working directory, which is the
    // user's project, not the emulator tree. Point it at the firmware
    // explicitly so `rosco run --emulator` works from anywhere.
    let rom_path = options
        .rom_path
        .clone()
        .or_else(|| default_rom_path(&options.program));
    if let Some(rom_path) = &rom_path {
        command.arg("-rompath").arg(rom_path);
    }
    // Image paths have to be absolute for the same reason.
    if let Some(binary) = &options.program_binary {
        command.arg("-quik").arg(absolute(binary));
    }
    if let Some(sd_card) = &options.sd_card {
        command.arg("-hard1").arg(absolute(sd_card));
    }
    command.args(&options.extra_args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().with_context(|| {
        let _ = std::fs::remove_dir_all(&scratch);
        format!(
            "could not start the emulator {}; \
             point --emulator-path, emulator.program in {}, or {} at it",
            options.program.display(),
            crate::config::CONFIG_FILE,
            PROGRAM_ENV,
        )
    })?;

    if let Some(stderr) = child.stderr.take() {
        relay_diagnostics(stderr);
    }

    let console = match accept_console(&listener, &mut child) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&scratch);
            return Err(error);
        }
    };

    Ok(EmulatorSession {
        child,
        console,
        scratch,
    })
}

/// Forwards the emulator's diagnostics, minus its warning about running
/// without video, which the console session makes moot but which would
/// otherwise appear on every start.
fn relay_diagnostics(stderr: std::process::ChildStderr) {
    const EXPECTED_NOISE: &str = "-video none doesn't make much sense";

    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
            if !line.contains(EXPECTED_NOISE) {
                eprintln!("{line}");
            }
        }
    });
}

/// Per-session directory for the files the emulator writes as it runs.
fn scratch_directory(port: u16) -> PathBuf {
    std::env::temp_dir().join(format!("rosco-cli-{}-{port}", std::process::id()))
}

fn accept_console(listener: &TcpListener, child: &mut Child) -> Result<TcpStream> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
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
                CONNECT_TIMEOUT.as_secs()
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
