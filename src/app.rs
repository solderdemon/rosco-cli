use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serialport::SerialPort;

use crate::build;
use crate::cli::{Cli, RoscoCommand, SerialArgs};
use crate::config::{Config, resolve_from};
use crate::kermit::{KermitOptions, TransferProgress};
use crate::serial;

pub fn run(cli: Cli) -> Result<()> {
    if let RoscoCommand::Init(args) = &cli.command {
        let destination = resolve_init_destination(&args.destination)?;
        crate::init::create(args.language.clone(), &destination)?;
        eprintln!("Created {}", destination.display());
        return Ok(());
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
        RoscoCommand::Init(_) => unreachable!("handled before resolving the project"),
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
            let (port_name, baud) = serial_settings(&config, &args.serial)?;
            let mut port = open_configured_port(&config, &port_name, baud)?;
            upload(&mut *port, &file, &config)?;
        }
        RoscoCommand::Monitor(args) => {
            let (port_name, baud) = serial_settings(&config, &args.serial)?;
            let port = open_configured_port(&config, &port_name, baud)?;
            serial::monitor(port)?;
        }
        RoscoCommand::Run(args) => {
            let output = build::build(&project_root, &config, args.clean)?;
            let (port_name, baud) = serial_settings(&config, &args.serial)?;
            let mut port = open_configured_port(&config, &port_name, baud)?;
            upload(&mut *port, &output.artifact, &config)?;
            serial::monitor(port)?;
        }
        RoscoCommand::Ports(args) => print_ports(args.all)?,
        RoscoCommand::Doctor => doctor(&config)?,
    }
    Ok(())
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
