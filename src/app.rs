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
    let project_root = absolute_project_root(&cli.project)?;
    if !project_root.is_dir() {
        bail!(
            "project directory does not exist: {}",
            project_root.display()
        );
    }
    let config = Config::load(&project_root)?;

    match cli.command {
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
        RoscoCommand::Ports => print_ports()?,
        RoscoCommand::Doctor => doctor(&config)?,
    }
    Ok(())
}

fn absolute_project_root(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("could not determine current directory")?
            .join(path))
    }
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
    let usb: Vec<_> = ports
        .iter()
        .filter(|port| port.kind.starts_with("USB"))
        .collect();
    if usb.len() == 1 {
        eprintln!("Auto-detected {} ({})", usb[0].name, usb[0].kind);
        return Ok(usb[0].name.clone());
    }
    if ports.len() == 1 {
        eprintln!("Auto-detected {} ({})", ports[0].name, ports[0].kind);
        return Ok(ports[0].name.clone());
    }
    if ports.is_empty() {
        bail!("no serial ports found; connect a UART adapter or pass --port");
    }

    let choices = ports
        .iter()
        .map(|port| format!("{} ({})", port.name, port.kind))
        .collect::<Vec<_>>()
        .join(", ");
    bail!("multiple serial ports found; pass --port. Available: {choices}")
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

fn print_ports() -> Result<()> {
    let ports = serial::list_ports()?;
    if ports.is_empty() {
        println!("No serial ports found.");
    } else {
        for port in ports {
            println!("{:<20} {}", port.name, port.kind);
        }
    }
    Ok(())
}

fn doctor(config: &Config) -> Result<()> {
    let builder_ok = probe_command(&config.build.program);
    print_check("build command", &config.build.program, builder_ok);

    let compiler = "m68k-elf-rosco-gcc";
    print_check("C toolchain", compiler, probe_command(compiler));

    match serial::list_ports() {
        Ok(ports) if ports.is_empty() => println!("[WARN] UART: no serial ports found"),
        Ok(ports) => println!("[ OK ] UART: {} serial port(s) found", ports.len()),
        Err(error) => println!("[WARN] UART: {error:#}"),
    }

    if !builder_ok {
        bail!(
            "configured build command `{}` is unavailable",
            config.build.program
        );
    }
    Ok(())
}

fn probe_command(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn print_check(label: &str, program: &str, available: bool) {
    let status = if available { " OK " } else { "WARN" };
    println!("[{status}] {label}: {program}");
}
