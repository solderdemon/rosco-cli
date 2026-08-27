use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Build, upload, and monitor programs for rosco_m68k.
#[derive(Debug, Parser)]
#[command(name = "rosco", version, propagate_version = true)]
pub struct Cli {
    /// rosco_m68k application directory.
    #[arg(short = 'C', long, global = true, default_value = ".")]
    pub project: PathBuf,

    #[command(subcommand)]
    pub command: RoscoCommand,
}

#[derive(Debug, Subcommand)]
pub enum RoscoCommand {
    /// Create a new C or assembly rosco_m68k project.
    Init(InitArgs),
    /// Compile the current application.
    Build(BuildArgs),
    /// Upload a binary through UART using Kermit.
    Upload(UploadArgs),
    /// Open an interactive UART session.
    Monitor(MonitorArgs),
    /// Build, upload, and open an interactive UART session.
    Run(RunArgs),
    /// List USB serial ports that may be connected to rosco_m68k.
    Ports(PortsArgs),
    /// Check Docker, local build tools, and UART setup.
    Doctor,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Project language.
    pub language: ProjectLanguage,

    /// Directory to create. It must not already exist.
    pub destination: PathBuf,
}

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum ProjectLanguage {
    C,
    Asm,
}

#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Run the configured clean command before building.
    #[arg(long)]
    pub clean: bool,
}

#[derive(Debug, Args)]
pub struct UploadArgs {
    /// Binary to upload; defaults to project.artifact in rosco.toml.
    pub file: Option<PathBuf>,

    #[command(flatten)]
    pub serial: SerialArgs,
}

#[derive(Debug, Args)]
pub struct MonitorArgs {
    #[command(flatten)]
    pub serial: SerialArgs,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Run the configured clean command before building.
    #[arg(long)]
    pub clean: bool,

    #[command(flatten)]
    pub serial: SerialArgs,
}

#[derive(Debug, Args)]
pub struct PortsArgs {
    /// Include built-in and non-USB serial ports such as /dev/ttyS*.
    #[arg(long)]
    pub all: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub struct SerialArgs {
    /// Serial device, for example /dev/ttyUSB0 or COM3.
    #[arg(short, long)]
    pub port: Option<String>,

    /// UART baud rate.
    #[arg(short, long)]
    pub baud: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn binary_is_named_rosco_and_parses_run_options() {
        let cli = Cli::try_parse_from([
            "rosco", "-C", "demo", "run", "--clean", "--port", "COM3", "--baud", "38400",
        ])
        .unwrap();

        assert_eq!(cli.project, PathBuf::from("demo"));
        let RoscoCommand::Run(args) = cli.command else {
            panic!("expected run command");
        };
        assert!(args.clean);
        assert_eq!(args.serial.port.as_deref(), Some("COM3"));
        assert_eq!(args.serial.baud, Some(38_400));
    }

    #[test]
    fn ports_hides_non_usb_devices_unless_all_is_requested() {
        let cli = Cli::try_parse_from(["rosco", "ports"]).unwrap();
        let RoscoCommand::Ports(args) = cli.command else {
            panic!("expected ports command");
        };
        assert!(!args.all);

        let cli = Cli::try_parse_from(["rosco", "ports", "--all"]).unwrap();
        let RoscoCommand::Ports(args) = cli.command else {
            panic!("expected ports command");
        };
        assert!(args.all);
    }

    #[test]
    fn parses_init_language_and_destination() {
        let cli = Cli::try_parse_from(["rosco", "init", "asm", "hello"]).unwrap();
        let RoscoCommand::Init(args) = cli.command else {
            panic!("expected init command");
        };
        assert!(matches!(args.language, ProjectLanguage::Asm));
        assert_eq!(args.destination, PathBuf::from("hello"));
    }
}
