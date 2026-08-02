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
    /// Compile the current application.
    Build(BuildArgs),
    /// Upload a binary through UART using Kermit.
    Upload(UploadArgs),
    /// Display UART output until interrupted.
    Monitor(MonitorArgs),
    /// Build, upload, and then display UART output.
    Run(RunArgs),
    /// List serial ports visible to the host.
    Ports,
    /// Check the local build toolchain and UART setup.
    Doctor,
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
}
