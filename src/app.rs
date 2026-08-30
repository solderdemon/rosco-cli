use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serialport::SerialPort;

use crate::build;
use crate::cli::{Cli, EmulatorArgs, RoscoCommand, SerialArgs};
use crate::config::{self, Config, Target, resolve_from};
use crate::emulator::{self, EmulatorOptions};
use crate::kermit::{KermitOptions, TransferProgress};
use crate::serial::{self, ConsoleKind};
use crate::settings;

pub fn run(cli: Cli) -> Result<()> {
    if let RoscoCommand::Init(args) = &cli.command {
        let destination = resolve_init_destination(&args.destination)?;
        crate::init::create(args.language.clone(), &destination)?;
        eprintln!("Created {}", destination.display());
        return Ok(());
    }

    if let RoscoCommand::Config(args) = cli.command {
        // Saving a user setting must work anywhere, project or not.
        let project_root = absolute_project_root(&cli.project).ok();
        return settings::run(project_root.as_deref(), args.command);
    }

    let project_root = absolute_project_root(&cli.project)?;
    if !project_root.is_dir() {
        bail!(
            "project directory does not exist: {}",
            project_root.display()
        );
    }
    let config = Config::load(&project_root)?;

    match cli.command {
        RoscoCommand::Init(_) | RoscoCommand::Config(_) => {
            unreachable!("handled before resolving the project")
        }
        RoscoCommand::Build(args) => {
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
                let options = emulator_options(&config, &args.emulator, None)?;
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
            let output = build::build(&project_root, &config, args.clean)?;

            if emulator {
                let options = emulator_options(&config, &args.emulator, Some(&output.artifact))?;
                // We built this with the 68k toolchain, so we know what it
                // can and cannot run on.
                require_machine_family(&options.machine, emulator::Family::M68k)?;
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

fn emulator_options(
    config: &Config,
    args: &EmulatorArgs,
    program_binary: Option<&Path>,
) -> Result<EmulatorOptions> {
    Ok(EmulatorOptions {
        program: emulator::resolve_program(config, args.emulator_path.as_deref())?,
        machine: args
            .machine
            .clone()
            .unwrap_or_else(|| config.emulator.machine.clone()),
        program_binary: program_binary.map(Path::to_path_buf),
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
    if let Some(binary) = &options.program_binary {
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

fn resolve_init_destination(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("could not determine current directory")?
            .join(path))
    }
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

fn upload(port: &mut dyn SerialPort, file: &Path, config: &Config) -> Result<()> {
    if !file.is_file() {
        bail!("binary does not exist: {}", file.display());
    }
    eprintln!("Uploading {}", file.display());
    let cancel = AtomicBool::new(false);
    let options = KermitOptions {
        max_retries: config.upload.max_retries,
        packet_timeout: Duration::from_millis(config.upload.packet_timeout_ms),
    };
    let mut last_percent = None;
    let name = crate::kermit::send_file(port, file, &cancel, options, |progress| {
        show_progress(&progress, &mut last_percent);
    })?;
    eprintln!("\rUploaded {name} successfully.                    ");
    Ok(())
}

fn show_progress(progress: &TransferProgress, last_percent: &mut Option<usize>) {
    let percent = progress
        .sent
        .saturating_mul(100)
        .checked_div(progress.total)
        .unwrap_or(100);
    if *last_percent != Some(percent) {
        eprint!(
            "\rUploading {}: {:>3}% ({}/{})",
            progress.name, percent, progress.sent, progress.total
        );
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
    let docker_ok = probe_docker();
    print_check("Docker", "docker", docker_ok);
    if docker_ok {
        let image_ok = docker_image_available(&config.build.docker.image);
        print_check("toolchain image", &config.build.docker.image, image_ok);
        if !image_ok {
            println!("[WARN] toolchain image will be pulled on the first build");
        }
    }
    print_check(
        "local build command",
        &config.build.program,
        probe_command(&config.build.program),
    );
    print_check(
        "local C toolchain",
        "m68k-elf-rosco-gcc",
        probe_command("m68k-elf-rosco-gcc"),
    );

    match emulator::resolve_program(config, None) {
        Ok(program) => print_check(
            "emulator",
            &program.display().to_string(),
            probe_emulator(&program),
        ),
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

fn docker_image_available(image: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn probe_docker() -> bool {
    Command::new("docker")
        .arg("info")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn probe_command(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
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
