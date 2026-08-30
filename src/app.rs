use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serialport::SerialPort;

use crate::build;
use crate::cli::{
    Cli, EmulatorArgs, InitArgs, ProjectLanguage, RoscoCommand, SerialArgs, ToolchainArgs,
};
use crate::config::{self, Board, Config, ProjectKind, Target, Toolchain, resolve_from};
use crate::emulator::{self, EmulatorOptions, Firmware};
use crate::ihex::{self, HexOptions};
use crate::init::{self, EmulatorSetup, ProjectPlan};
use crate::kermit::KermitOptions;
use crate::prompt::{self, Choice};
use crate::serial::{self, ConsoleKind};
use crate::settings;
use crate::toolchain;

pub fn run(cli: Cli) -> Result<()> {
    let Cli { project, command } = cli;

    // Neither of these needs a project on disk, and `config` has to work
    // outside one to save a setting for this user.
    let command = match command {
        RoscoCommand::Init(args) => return init(args, &project),
        RoscoCommand::Config(args) => {
            let project_root = absolute_project_root(&project).ok();
            return settings::run(project_root.as_deref(), args.command);
        }
        other => other,
    };

    let project_root = absolute_project_root(&project)?;
    if !project_root.is_dir() {
        bail!(
            "project directory does not exist: {}",
            project_root.display()
        );
    }
    let mut config = Config::load(&project_root)?;

    match command {
        RoscoCommand::Init(_) | RoscoCommand::Config(_) => {
            unreachable!("handled before resolving the project")
        }
        RoscoCommand::Build(args) => {
            choose_toolchain(&mut config, &args.toolchain);
            let output = build::build(&project_root, &config, args.clean)?;
            eprintln!("Built {}", output.artifact.display());
        }
        RoscoCommand::Upload(args) => {
            let file = args
                .file
                .map(|path| resolve_from(&project_root, &path))
                .map(Ok)
                .unwrap_or_else(|| config.artifact_path(&project_root))?;
            if !file.is_file() {
                bail!("binary does not exist: {}", file.display());
            }

            if use_emulator(&config, &args.emulator)? {
                // Nothing persists between emulator runs, so loading a
                // binary and running it are the same thing.
                let options = emulator_options(&config, &args.emulator, Some(&file))?;
                attach_emulator(&options)?;
            } else {
                let (port_name, baud) = serial_settings(&config, &args.serial)?;
                let mut port = open_configured_port(&config, &port_name, baud)?;
                upload(&mut *port, &file, &config)?;
            }
        }
        RoscoCommand::Monitor(args) => {
            if use_emulator(&config, &args.emulator)? {
                // In a firmware project the machine is the project, so there
                // is something to boot even with nothing to load into it.
                let firmware = built_firmware(&config, &project_root)?;
                let options = emulator_options(&config, &args.emulator, firmware.as_deref())?;
                attach_emulator(&options)?;
            } else {
                let (port_name, baud) = serial_settings(&config, &args.serial)?;
                let port = open_configured_port(&config, &port_name, baud)?;
                serial::monitor(port)?;
            }
        }
        RoscoCommand::Run(args) => {
            // Resolved before the build so a bad flag fails in a second
            // rather than after a toolchain run.
            let emulator = use_emulator(&config, &args.emulator)?;
            choose_toolchain(&mut config, &args.toolchain);
            let output = build::build(&project_root, &config, args.clean)?;

            if emulator {
                let options = emulator_options(&config, &args.emulator, Some(&output.artifact))?;
                // We built this ourselves, so we know what the result can and
                // cannot run on.
                require_machine_family(
                    &options.machine,
                    emulator::board_family(config.project.board),
                )?;
                attach_emulator(&options)?;
            } else {
                let (port_name, baud) = serial_settings(&config, &args.serial)?;
                let mut port = open_configured_port(&config, &port_name, baud)?;
                upload(&mut *port, &output.artifact, &config)?;
                serial::monitor(port)?;
            }
        }
        RoscoCommand::Ports(args) => print_ports(args.all)?,
        RoscoCommand::Doctor => doctor(&config)?,
    }
    Ok(())
}

/// Whether this run targets the emulator: the flags decide, and failing that
/// the saved `defaults.target`.
fn use_emulator(config: &Config, args: &EmulatorArgs) -> Result<bool> {
    let emulator = if args.hardware {
        false
    } else if args.emulator {
        true
    } else {
        config.defaults.target == Target::Emulator
    };

    // A board takes its firmware from a ROM chip, and there is no way to put
    // one there down a serial cable.
    if !emulator && config.project.kind == ProjectKind::Firmware {
        bail!(
            "this project builds firmware, which a board reads from its ROM rather than \
             over the UART.\n  Run it here with `rosco run --emulator`, or burn the image \
             to an EEPROM and put it in the socket."
        );
    }

    if !emulator {
        for (flag, given) in [
            ("--machine", args.machine.is_some()),
            ("--rom-path", args.rom_path.is_some()),
            ("--sd-card", args.sd_card.is_some()),
        ] {
            if given {
                bail!(
                    "{flag} only applies to the emulator; pass --emulator or run \
                     `rosco config set defaults.target emulator`"
                );
            }
        }
    }
    Ok(emulator)
}

/// Sets up an emulator run. `artifact` is whatever the project builds, which
/// in a firmware project is the machine itself rather than something loaded
/// into one.
fn emulator_options(
    config: &Config,
    args: &EmulatorArgs,
    artifact: Option<&Path>,
) -> Result<EmulatorOptions> {
    let firmware = match config.project.kind {
        ProjectKind::Firmware => artifact.map(|image| Firmware {
            image: image.to_path_buf(),
            rom_file: config.project.board.rom_file().to_string(),
        }),
        ProjectKind::Program => None,
    };
    Ok(EmulatorOptions {
        runner: emulator::resolve_runner(config, args.emulator_path.as_deref())?,
        machine: args
            .machine
            .clone()
            .unwrap_or_else(|| config.emulator.machine.clone()),
        program_binary: match config.project.kind {
            ProjectKind::Program => artifact.map(Path::to_path_buf),
            ProjectKind::Firmware => None,
        },
        firmware,
        rom_path: args
            .rom_path
            .clone()
            .or_else(|| config.emulator.rom_path.clone()),
        sd_card: args
            .sd_card
            .clone()
            .or_else(|| config.emulator.sd_card.clone()),
        extra_args: config.emulator.args.clone(),
    })
}

fn attach_emulator(options: &EmulatorOptions) -> Result<()> {
    if let Some(firmware) = &options.firmware {
        eprintln!(
            "Starting {} on {}",
            options.machine,
            firmware.image.display()
        );
    } else if let Some(binary) = &options.program_binary {
        eprintln!("Loading {} into {}", binary.display(), options.machine);
    } else {
        eprintln!("Starting {}", options.machine);
    }

    let session = emulator::start(options)?;
    let stream = session.console()?;
    // Time reads out so Ctrl-C is noticed promptly.
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .context("could not set a read timeout on the emulator console")?;
    let writer = session.console()?;

    serial::console(Box::new(stream), Box::new(writer), ConsoleKind::Emulator)
}

/// The firmware a project has already built, for the commands that do not
/// build one themselves.
fn built_firmware(config: &Config, project_root: &Path) -> Result<Option<PathBuf>> {
    if config.project.kind != ProjectKind::Firmware {
        return Ok(None);
    }

    let image = config.artifact_path(project_root)?;
    if !image.is_file() {
        bail!(
            "no firmware image at {}; `rosco build` makes one",
            image.display()
        );
    }
    Ok(Some(image))
}

/// Refuses a machine the artifact cannot possibly run on.
///
/// `upload` takes an arbitrary file and has no way to tell what it is, so it
/// trusts the caller. `run` built the binary itself and does know.
fn require_machine_family(machine: &str, expected: emulator::Family) -> Result<()> {
    match emulator::family(machine) {
        Some(family) if family == expected => Ok(()),
        Some(family) => bail!(
            "this project builds {expected} code, but {machine} is a {family} machine; \
             set emulator.machine in rosco.toml or pass --machine"
        ),
        None => bail!("unknown machine {machine}; expected a rosco_m68k or rosco_6502 machine"),
    }
}

/// Which compilers this run uses, when a flag disagrees with the settings.
fn choose_toolchain(config: &mut Config, args: &ToolchainArgs) {
    if args.docker {
        config.build.toolchain = Toolchain::Docker;
    } else if args.host {
        config.build.toolchain = Toolchain::Host;
    }
}

/// Creates a project, asking about anything the command line did not settle.
///
/// Every question has an option of its own, so `--yes` or a script's pipe
/// gets a project without a conversation.
fn init(args: InitArgs, parent: &Path) -> Result<()> {
    let (mut name, language_first) = split_positionals(&args.name)?;
    let mut language = args.language.or(language_first);
    let mut board = args.board;
    let mut kind = args.kind;
    let mut toolchain = requested_toolchain(&args.toolchain);
    let mut target = args.target;
    let mut emulator = requested_emulator(&args);

    let asking = !args.yes && prompt::is_interactive();
    // Only worth reading when there are questions left for it to answer.
    let remembered = asking.then(init::Remembered::load).flatten();

    if asking {
        prompt::heading("Create a rosco project");
        if name.is_none() {
            name = Some(prompt::text("Project name", DEFAULT_PROJECT_NAME)?);
        }

        let anything_to_ask = board.is_none()
            || kind.is_none()
            || language.is_none()
            || toolchain.is_none()
            || target.is_none()
            || emulator.is_none();
        let previous = match &remembered {
            Some(previous) if anything_to_ask && use_previous(previous)? => Some(previous),
            _ => None,
        };

        if let Some(previous) = previous {
            board = board.or(Some(previous.board));
            kind = kind.or(Some(previous.kind));
            language = language.or(Some(previous.language));
            toolchain = toolchain.or(Some(previous.toolchain));
            target = target.or(Some(previous.target));
            emulator = emulator.or_else(|| Some(previous.emulator()));
        } else {
            // Whatever was answered last time starts selected, so the run
            // that changes one thing is Enter, Enter, the change, Enter.
            let last = remembered.as_ref();
            if board.is_none() {
                let start = starting_at(BOARDS, last.map(|last| last.board));
                board = Some(BOARDS[prompt::select("Board", BOARD_CHOICES, start)?]);
            }
            if kind.is_none() {
                let start = starting_at(KINDS, last.map(|last| last.kind));
                kind = Some(KINDS[prompt::select("Runs as", KIND_CHOICES, start)?]);
            }
            // A firmware is assembly that owns the machine from its reset
            // vector, and a board only takes one from its ROM socket, so
            // neither of the next two questions has an answer to give.
            let firmware = kind == Some(ProjectKind::Firmware);
            if language.is_none() && !firmware {
                let start = starting_at(LANGUAGES, last.map(|last| last.language));
                language = Some(LANGUAGES[prompt::select("Language", LANGUAGE_CHOICES, start)?]);
            }
            if toolchain.is_none() {
                let start = starting_at(TOOLCHAINS, last.map(|last| last.toolchain));
                toolchain =
                    Some(TOOLCHAINS[prompt::select("Build with", TOOLCHAIN_CHOICES, start)?]);
            }
            if target.is_none() && !firmware {
                let start = starting_at(TARGETS, last.map(|last| last.target));
                target = Some(TARGETS[prompt::select("Run on", TARGET_CHOICES, start)?]);
            }
            if emulator.is_none() {
                emulator = Some(ask_emulator(last)?);
            }
        }
        eprintln!();
    }

    let name = match name {
        Some(name) => name,
        None => bail!(
            "no project name; `rosco init hello` creates one, and `rosco init --help` \
             lists what else can be chosen"
        ),
    };
    let kind = kind.unwrap_or_default();
    let plan = ProjectPlan {
        board: board.unwrap_or_default(),
        kind,
        // A firmware project has no choice about either of these: it is
        // assembly, and only the emulator here can run it.
        language: match kind {
            ProjectKind::Firmware => ProjectLanguage::Asm,
            ProjectKind::Program => language.unwrap_or(ProjectLanguage::C),
        },
        toolchain: toolchain.unwrap_or_default(),
        target: match kind {
            ProjectKind::Firmware => Target::Emulator,
            ProjectKind::Program => target.unwrap_or_default(),
        },
        emulator: emulator.unwrap_or_default(),
    };

    let destination = resolve_init_destination(parent, Path::new(&name))?;
    init::create(&plan, &destination)?;
    eprintln!("Created {}", destination.display());
    eprintln!("  {}", plan.describe());

    // Remembering is a courtesy, so a settings directory that cannot be
    // written is worth a word but not a failed `init`.
    if let Err(error) = init::Remembered::of(&plan).save() {
        eprintln!("  (these choices could not be remembered: {error:#})");
    }

    report_next_steps(&plan, &name);
    Ok(())
}

/// Offers the last run's answers instead of asking the same questions again.
fn use_previous(previous: &init::Remembered) -> Result<bool> {
    let summary = previous.summary();
    let choices = [
        Choice {
            label: "Previous setup",
            detail: &summary,
        },
        Choice {
            label: "New setup",
            detail: "answer the questions again",
        },
    ];
    Ok(prompt::select("Setup", &choices, 0)? == 0)
}

/// Where the emulator comes from, which is otherwise a flag on the end of
/// every command that runs anything.
fn ask_emulator(previous: Option<&init::Remembered>) -> Result<EmulatorSetup> {
    let last = previous.map(init::Remembered::emulator);
    let start = match last {
        Some(EmulatorSetup::Docker) => 0,
        Some(EmulatorSetup::Program(_)) => 1,
        Some(EmulatorSetup::EachRun) => 2,
        None => 0,
    };

    match prompt::select("Emulator", EMULATOR_CHOICES, start)? {
        0 => Ok(EmulatorSetup::Docker),
        1 => {
            let suggestion = match last {
                Some(EmulatorSetup::Program(path)) => path.display().to_string(),
                _ => std::env::var(emulator::PROGRAM_ENV)
                    .unwrap_or_else(|_| emulator::DEFAULT_PROGRAM.to_string()),
            };
            let answer = prompt::text("Emulator path", &suggestion)?;
            Ok(EmulatorSetup::Program(PathBuf::from(answer)))
        }
        _ => Ok(EmulatorSetup::EachRun),
    }
}

/// Which entry of a question a remembered answer sits at, so it can be the
/// one already selected when the question is drawn.
fn starting_at<T: PartialEq>(options: &[T], remembered: Option<T>) -> usize {
    remembered
        .and_then(|answer| options.iter().position(|option| *option == answer))
        .unwrap_or(0)
}

const DEFAULT_PROJECT_NAME: &str = "hello";

const BOARDS: &[Board] = &[Board::RoscoM68k, Board::Rosco6502];
const BOARD_CHOICES: &[Choice] = &[
    Choice {
        label: "rosco_m68k",
        detail: "68010 board, C or assembly through the m68k toolchain",
    },
    Choice {
        label: "rosco_6502",
        detail: "65C02 board, C or assembly through the cc65 tools",
    },
];

const LANGUAGES: &[ProjectLanguage] = &[ProjectLanguage::C, ProjectLanguage::Asm];
const LANGUAGE_CHOICES: &[Choice] = &[
    Choice {
        label: "C",
        detail: "a main() and the C library",
    },
    Choice {
        label: "Assembly",
        detail: "the machine and nothing between you and it",
    },
];

const TOOLCHAINS: &[Toolchain] = &[Toolchain::Docker, Toolchain::Host];
const TOOLCHAIN_CHOICES: &[Choice] = &[
    Choice {
        label: "Docker",
        detail: "the toolchain image; nothing to install",
    },
    Choice {
        label: "This computer",
        detail: "compilers on PATH; no container to start",
    },
];

const KINDS: &[ProjectKind] = &[ProjectKind::Program, ProjectKind::Firmware];
const KIND_CHOICES: &[Choice] = &[
    Choice {
        label: "A program",
        detail: "the firmware boots the board, then loads and runs it",
    },
    Choice {
        label: "The firmware",
        detail: "your code owns the machine from its reset vector",
    },
];

const TARGETS: &[Target] = &[Target::Hardware, Target::Emulator];
const TARGET_CHOICES: &[Choice] = &[
    Choice {
        label: "Hardware",
        detail: "a board on the other end of a UART",
    },
    Choice {
        label: "Emulator",
        detail: "the rosco emulator, with no hardware attached",
    },
];

/// The three answers `EmulatorSetup` has, in the same order.
const EMULATOR_CHOICES: &[Choice] = &[
    Choice {
        label: "Docker",
        detail: "the emulator image; nothing to install",
    },
    Choice {
        label: "This computer",
        detail: "a build of it, saved in the project so no flag is needed",
    },
    Choice {
        label: "Decide each run",
        detail: "ROSCO_EMULATOR, or rosco-emulator on PATH",
    },
];

/// What the command line said about the emulator, if anything.
fn requested_emulator(args: &InitArgs) -> Option<EmulatorSetup> {
    if let Some(path) = &args.emulator_path {
        return Some(EmulatorSetup::Program(path.clone()));
    }
    args.emulator_docker.then_some(EmulatorSetup::Docker)
}

fn requested_toolchain(args: &ToolchainArgs) -> Option<Toolchain> {
    match (args.docker, args.host) {
        (true, _) => Some(Toolchain::Docker),
        (_, true) => Some(Toolchain::Host),
        _ => None,
    }
}

/// Reads `init`'s positional arguments.
///
/// One argument names the project. Two are the older `init <language> <name>`
/// form, which is still what the documentation of released versions says.
fn split_positionals(positionals: &[String]) -> Result<(Option<String>, Option<ProjectLanguage>)> {
    match positionals {
        [] => Ok((None, None)),
        [name] => Ok((Some(name.clone()), None)),
        [language, name] => {
            let language = LANGUAGES
                .iter()
                .find(|candidate| candidate.label() == language.to_ascii_lowercase())
                .copied()
                .with_context(|| {
                    format!(
                        "{language} is not a language; write `rosco init {name} --language c` \
                         or `rosco init c {name}`"
                    )
                })?;
            Ok((Some(name.clone()), Some(language)))
        }
        _ => bail!("too many names; `rosco init hello` creates one project"),
    }
}

/// The two or three commands that take the new project somewhere.
fn report_next_steps(plan: &ProjectPlan, name: &str) {
    let missing = match plan.toolchain {
        Toolchain::Host => toolchain::missing(plan.board),
        Toolchain::Docker => Vec::new(),
    };
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|tool| tool.command).collect();
        eprintln!("\nNot on PATH yet: {}", names.join(", "));
        if let Some(hint) = toolchain::install_hint(plan.board) {
            eprintln!("  {hint}");
        }
    }

    // A path that is not there yet is worth saying out loud now rather than
    // when the first run cannot start it.
    if let EmulatorSetup::Program(path) = &plan.emulator
        && !emulator_present(path)
    {
        eprintln!("\nNo emulator at {} yet.", path.display());
        eprintln!(
            "  `rosco config set --local emulator.program <path>` points the project at one."
        );
    }

    eprintln!("\nNext:");
    eprintln!("  cd {name}");
    match (plan.kind, plan.target) {
        (ProjectKind::Firmware, _) => {
            eprintln!("  rosco run                        # boots the machine on your ROM");
            eprintln!("\nA board reads its firmware from a ROM chip, so this one runs in the");
            eprintln!("emulator. Burning the image to an EEPROM is what puts it on hardware.");
        }
        (_, Target::Emulator) => eprintln!("  rosco run"),
        (_, Target::Hardware) => {
            eprintln!("  rosco run --port /dev/ttyUSB0    # or COM3 on Windows");
            eprintln!("  rosco run --emulator             # with no board on the desk");
        }
    }
}

/// Whether the recorded emulator is there: a path names a file, and a bare
/// name is looked up the way running it would.
fn emulator_present(path: &Path) -> bool {
    if path.components().count() > 1 {
        return path.is_file();
    }
    path.is_file() || toolchain::available(&path.display().to_string())
}

/// Where the new project goes: `-C` names the directory to create it in, and
/// defaults to this one.
fn resolve_init_destination(parent: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()
        .context("could not determine current directory")?
        .join(parent)
        .join(path)
        .components()
        .collect())
}

fn absolute_project_root(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("could not determine current directory")?
            .join(path)
    }
    .canonicalize()
    .with_context(|| format!("could not resolve project directory {}", path.display()))
}

fn serial_settings(config: &Config, args: &SerialArgs) -> Result<(String, u32)> {
    let name = match args.port.clone().or_else(|| config.serial.port.clone()) {
        Some(name) => name,
        None => auto_detect_port()?,
    };
    Ok((name, args.baud.unwrap_or(config.serial.baud)))
}

fn auto_detect_port() -> Result<String> {
    let ports = serial::list_ports()?;
    auto_detect_port_from(&ports)
}

fn auto_detect_port_from(ports: &[serial::PortSummary]) -> Result<String> {
    if ports.is_empty() {
        bail!("no serial ports found; connect a UART adapter or pass --port");
    }

    let usb: Vec<_> = ports.iter().filter(|port| port.is_usb).collect();
    let candidates: Vec<_> = if usb.is_empty() {
        ports.iter().collect()
    } else {
        usb
    };

    if candidates.len() == 1 {
        eprintln!(
            "Auto-detected {} ({})",
            candidates[0].name, candidates[0].kind
        );
        return Ok(candidates[0].name.clone());
    }

    let choices = candidates
        .iter()
        .map(|port| format!("{} ({})", port.name, port.kind))
        .collect::<Vec<_>>()
        .join(", ");
    bail!("multiple candidate serial ports found; pass --port. Available: {choices}")
}

fn open_configured_port(config: &Config, name: &str, baud: u32) -> Result<Box<dyn SerialPort>> {
    serial::open(
        name,
        baud,
        Duration::from_millis(config.serial.read_timeout_ms),
    )
}

/// Sends a binary to a board over the UART, by whichever route its firmware
/// understands: Kermit on the m68k, Intel hex through the monitor on the 6502.
fn upload(port: &mut dyn SerialPort, file: &Path, config: &Config) -> Result<()> {
    if !file.is_file() {
        bail!("binary does not exist: {}", file.display());
    }
    eprintln!("Uploading {}", file.display());
    let mut last_percent = None;

    let name = match config.project.board {
        Board::RoscoM68k => {
            let cancel = AtomicBool::new(false);
            let options = KermitOptions {
                max_retries: config.upload.max_retries,
                packet_timeout: Duration::from_millis(config.upload.packet_timeout_ms),
            };
            crate::kermit::send_file(port, file, &cancel, options, |progress| {
                show_progress(
                    &progress.name,
                    progress.sent,
                    progress.total,
                    &mut last_percent,
                );
            })?
        }
        Board::Rosco6502 => {
            let bytes = std::fs::read(file)
                .with_context(|| format!("could not read {}", file.display()))?;
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "PROGRAM.BIN".into());
            let options = HexOptions {
                load_address: load_address(config)?,
                reply_timeout: Duration::from_millis(config.upload.packet_timeout_ms),
                max_retries: config.upload.max_retries,
            };
            ihex::send_program(port, &name, &bytes, options, |progress| {
                show_progress(
                    &progress.name,
                    progress.sent,
                    progress.total,
                    &mut last_percent,
                );
            })?;
            name
        }
    };

    eprintln!("\rUploaded {name} successfully.                    ");
    Ok(())
}

/// The 6502 has 64K of address space and nothing outside it to load into.
fn load_address(config: &Config) -> Result<u16> {
    u16::try_from(config.upload.load_address).map_err(|_| {
        anyhow::anyhow!(
            "upload.load_address is ${:X}, which is outside the 6502's address space",
            config.upload.load_address
        )
    })
}

fn show_progress(name: &str, sent: usize, total: usize, last_percent: &mut Option<usize>) {
    let percent = sent.saturating_mul(100).checked_div(total).unwrap_or(100);
    if *last_percent != Some(percent) {
        eprint!("\rUploading {name}: {percent:>3}% ({sent}/{total})");
        *last_percent = Some(percent);
    }
}

fn print_ports(show_all: bool) -> Result<()> {
    let ports = serial::list_ports()?;
    if ports.is_empty() {
        println!("No serial ports found.");
        return Ok(());
    }

    let visible: Vec<_> = ports
        .iter()
        .filter(|port| show_all || port.is_usb)
        .collect();
    if visible.is_empty() {
        println!(
            "No USB serial ports found ({} non-USB port(s) hidden; use --all to show them).",
            ports.len()
        );
        return Ok(());
    }

    for port in &visible {
        println!("{:<20} {}", port.name, port.kind);
    }
    if !show_all && visible.len() == 1 {
        println!(
            "\nAuto-selected when --port is omitted: {}",
            visible[0].name
        );
    }
    Ok(())
}

fn doctor(config: &Config) -> Result<()> {
    let board = config.project.board;
    println!(
        "[ -- ] project: {board}, built with the {} toolchain",
        config.build.toolchain
    );

    match build::ensure_docker_available() {
        Ok(()) => {
            print_check("Docker", "docker", true);
            let image = &config.build.docker.image;
            let pulled = build::docker_image_available(image);
            print_check("toolchain image", image, pulled);
            if !pulled {
                println!("[ -- ] the image is pulled on the first build");
            }
        }
        Err(error) => println!("[WARN] Docker: {error:#}"),
    }

    let mut absent = false;
    for tool in toolchain::required(board) {
        let available = toolchain::available(tool.command);
        absent |= !available;
        print_check(tool.purpose, tool.command, available);
    }
    if absent && let Some(hint) = toolchain::install_hint(board) {
        println!("[ -- ] a build on this computer needs: {hint}");
    }

    match emulator::resolve_runner(config, None) {
        Ok(emulator::Runner::Program(program)) => print_check(
            "emulator",
            &program.display().to_string(),
            probe_emulator(&program),
        ),
        // Nothing is downloaded to find out: an image that is not here yet is
        // pulled by the run that needs it.
        Ok(emulator::Runner::Image { name, .. }) => {
            let pulled = build::docker_image_available(&name);
            print_check("emulator image", &name, pulled);
            if !pulled {
                println!("[ -- ] the image is pulled on the first emulator run");
            }
        }
        Err(error) => println!("[WARN] emulator: {error:#}"),
    }

    match config::global_config_path() {
        Ok(path) if path.is_file() => {
            print_check("user settings", &path.display().to_string(), true)
        }
        Ok(path) => println!("[ -- ] user settings: {} (not created yet)", path.display()),
        Err(error) => println!("[WARN] user settings: {error:#}"),
    }

    match serial::list_ports() {
        Ok(ports) if ports.is_empty() => println!("[WARN] UART: no serial ports found"),
        Ok(ports) => {
            let usb_count = ports.iter().filter(|port| port.is_usb).count();
            if usb_count == 0 {
                println!(
                    "[WARN] UART: no USB serial ports found ({} non-USB port(s) ignored)",
                    ports.len()
                );
            } else {
                println!("[ OK ] UART: {usb_count} USB serial port(s) found");
            }
        }
        Err(error) => println!("[WARN] UART: {error:#}"),
    }

    Ok(())
}

fn probe_emulator(program: &Path) -> bool {
    Command::new(program)
        .arg("-help")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn print_check(label: &str, program: &str, available: bool) {
    let status = if available { " OK " } else { "WARN" };
    println!("[{status}] {label}: {program}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, kind: &str, is_usb: bool) -> serial::PortSummary {
        serial::PortSummary {
            name: name.into(),
            kind: kind.into(),
            is_usb,
        }
    }

    #[test]
    fn a_68k_project_refuses_to_run_on_the_6502_machine() {
        let error = require_machine_family("rosco_6502", emulator::Family::M68k)
            .unwrap_err()
            .to_string();
        assert!(error.contains("builds 68k code"), "{error}");
        assert!(error.contains("rosco_6502"), "{error}");
    }

    #[test]
    fn every_68k_variant_is_accepted_because_they_run_the_same_binary() {
        for machine in [
            "rosco_m68k_000",
            "rosco_m68k_010",
            "rosco_m68k_020",
            "rosco_m68k_030",
        ] {
            require_machine_family(machine, emulator::Family::M68k).unwrap();
        }
    }

    #[test]
    fn an_unrecognised_machine_is_rejected() {
        let error = require_machine_family("commodore64", emulator::Family::M68k)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown machine"), "{error}");
    }

    #[test]
    fn the_saved_target_decides_when_no_flag_is_passed() {
        let mut config = Config::default();
        let args = EmulatorArgs::default();
        assert!(!use_emulator(&config, &args).unwrap());

        config.defaults.target = Target::Emulator;
        assert!(use_emulator(&config, &args).unwrap());
    }

    #[test]
    fn a_firmware_project_cannot_be_sent_to_a_board() {
        let mut config = Config::default();
        config.project.kind = ProjectKind::Firmware;

        let error = use_emulator(&config, &EmulatorArgs::default())
            .unwrap_err()
            .to_string();

        assert!(error.contains("reads from its ROM"), "{error}");
        assert!(error.contains("--emulator"), "{error}");
    }

    #[test]
    fn a_firmware_project_is_the_machine_rather_than_something_loaded_into_it() {
        let mut config = Config::default();
        config.project.board = Board::Rosco6502;
        config.project.kind = ProjectKind::Firmware;

        let options = emulator_options(
            &config,
            &EmulatorArgs::default(),
            Some(Path::new("/work/hello/hello.rom")),
        )
        .unwrap();

        assert!(options.program_binary.is_none());
        let firmware = options.firmware.expect("the image runs as the firmware");
        assert_eq!(firmware.image, Path::new("/work/hello/hello.rom"));
        assert_eq!(firmware.rom_file, "rosco_6502.rom");
    }

    #[test]
    fn hardware_beats_a_saved_emulator_default() {
        let mut config = Config::default();
        config.defaults.target = Target::Emulator;
        let args = EmulatorArgs {
            hardware: true,
            ..EmulatorArgs::default()
        };

        assert!(!use_emulator(&config, &args).unwrap());
    }

    #[test]
    fn emulator_only_flags_are_refused_when_targeting_hardware() {
        let args = EmulatorArgs {
            machine: Some("rosco_6502".into()),
            ..EmulatorArgs::default()
        };

        let error = use_emulator(&Config::default(), &args)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--machine"), "{error}");
    }

    #[test]
    fn a_6502_project_refuses_to_run_on_an_m68k_machine() {
        let error =
            require_machine_family("rosco_m68k_010", emulator::board_family(Board::Rosco6502))
                .unwrap_err()
                .to_string();
        assert!(error.contains("builds 6502 code"), "{error}");
    }

    fn init_args() -> InitArgs {
        InitArgs {
            name: Vec::new(),
            board: None,
            kind: None,
            language: None,
            target: None,
            emulator_docker: false,
            emulator_path: None,
            toolchain: ToolchainArgs::default(),
            yes: false,
        }
    }

    #[test]
    fn nothing_said_about_the_emulator_writes_nothing_down() {
        assert_eq!(requested_emulator(&init_args()), None);
    }

    #[test]
    fn the_emulator_can_be_settled_on_the_command_line_either_way() {
        assert_eq!(
            requested_emulator(&InitArgs {
                emulator_docker: true,
                ..init_args()
            }),
            Some(EmulatorSetup::Docker)
        );
        assert_eq!(
            requested_emulator(&InitArgs {
                emulator_path: Some("/opt/rosco/rosco".into()),
                ..init_args()
            }),
            Some(EmulatorSetup::Program("/opt/rosco/rosco".into()))
        );
    }

    #[test]
    fn a_question_opens_on_the_answer_it_was_given_last_time() {
        assert_eq!(starting_at(BOARDS, Some(Board::Rosco6502)), 1);
        assert_eq!(starting_at(TARGETS, Some(Target::Hardware)), 0);
        // Nothing remembered leaves the first entry selected.
        assert_eq!(starting_at(TOOLCHAINS, None), 0);
    }

    #[test]
    fn every_question_offers_exactly_as_many_answers_as_it_has() {
        assert_eq!(BOARDS.len(), BOARD_CHOICES.len());
        assert_eq!(LANGUAGES.len(), LANGUAGE_CHOICES.len());
        assert_eq!(TOOLCHAINS.len(), TOOLCHAIN_CHOICES.len());
        assert_eq!(TARGETS.len(), TARGET_CHOICES.len());
        assert_eq!(KINDS.len(), KIND_CHOICES.len());
        // The emulator question answers with a value built where it is asked,
        // and `ask_emulator` matches on all three of them.
        assert_eq!(EMULATOR_CHOICES.len(), 3);
    }

    #[test]
    fn a_bare_command_name_counts_as_present_when_it_can_be_run() {
        // Nothing is asserted about this machine's PATH, only that a name is
        // looked up rather than being called a missing file.
        assert!(!emulator_present(Path::new("/nowhere/at/all/rosco")));
        assert_eq!(
            emulator_present(Path::new("cargo")),
            toolchain::available("cargo")
        );
    }

    #[test]
    fn one_name_is_the_project_and_two_are_the_older_form() {
        assert_eq!(
            split_positionals(&["hello".to_string()]).unwrap(),
            (Some("hello".to_string()), None)
        );

        let (name, language) =
            split_positionals(&["asm".to_string(), "hello".to_string()]).unwrap();
        assert_eq!(name.as_deref(), Some("hello"));
        assert_eq!(language, Some(ProjectLanguage::Asm));
    }

    #[test]
    fn a_first_word_that_is_not_a_language_says_what_both_forms_look_like() {
        let error = split_positionals(&["hullo".to_string(), "hello".to_string()])
            .unwrap_err()
            .to_string();

        assert!(error.contains("--language c"), "{error}");
        assert!(error.contains("rosco init c hello"), "{error}");
    }

    #[test]
    fn a_flag_beats_the_saved_toolchain_for_one_run() {
        let mut config = Config::default();
        config.build.toolchain = Toolchain::Docker;

        choose_toolchain(
            &mut config,
            &ToolchainArgs {
                host: true,
                docker: false,
            },
        );
        assert_eq!(config.build.toolchain, Toolchain::Host);

        // Nothing passed leaves the settings in charge.
        choose_toolchain(&mut config, &ToolchainArgs::default());
        assert_eq!(config.build.toolchain, Toolchain::Host);
    }

    #[test]
    fn auto_detection_ignores_linux_builtin_ports_when_one_usb_port_exists() {
        let ports = [
            port("/dev/ttyS0", "PCI", false),
            port("/dev/ttyS1", "PCI", false),
            port("/dev/ttyUSB0", "USB 0403:6001", true),
        ];

        assert_eq!(auto_detect_port_from(&ports).unwrap(), "/dev/ttyUSB0");
    }

    #[test]
    fn auto_detection_only_lists_usb_candidates_when_there_are_several() {
        let ports = [
            port("/dev/ttyS0", "PCI", false),
            port("/dev/ttyUSB0", "USB 0403:6001", true),
            port("/dev/ttyACM0", "USB 10c4:ea60", true),
        ];

        let error = auto_detect_port_from(&ports).unwrap_err().to_string();
        assert!(error.contains("/dev/ttyUSB0"));
        assert!(error.contains("/dev/ttyACM0"));
        assert!(!error.contains("/dev/ttyS0"));
    }
}
