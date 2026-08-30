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
    /// Read and write saved settings.
    Config(ConfigArgs),
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

    #[command(flatten)]
    pub emulator: EmulatorArgs,
}

#[derive(Debug, Args)]
pub struct MonitorArgs {
    #[command(flatten)]
    pub serial: SerialArgs,

    #[command(flatten)]
    pub emulator: EmulatorArgs,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Run the configured clean command before building.
    #[arg(long)]
    pub clean: bool,

    #[command(flatten)]
    pub serial: SerialArgs,

    #[command(flatten)]
    pub emulator: EmulatorArgs,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Show every setting in force and which file supplies it.
    #[command(alias = "show")]
    List,
    /// Print one setting's value.
    Get {
        /// Setting name, for example serial.port.
        key: String,
    },
    /// Save a setting so it no longer has to be passed as an option.
    Set {
        /// Setting name, for example serial.port.
        key: String,
        /// Value to store.
        value: String,

        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Remove a saved setting.
    Unset {
        /// Setting name, for example serial.port.
        key: String,

        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Print the path of a settings file.
    Path {
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Open a settings file in $EDITOR.
    Edit {
        #[command(flatten)]
        scope: ScopeArgs,
    },
}

#[derive(Clone, Copy, Debug, Default, Args)]
pub struct ScopeArgs {
    /// Use the per-user settings file shared by every project (the default).
    #[arg(long, group = "settings-scope")]
    pub global: bool,

    /// Use rosco.toml in the project directory.
    #[arg(long, group = "settings-scope")]
    pub local: bool,
}

#[derive(Debug, Args)]
pub struct PortsArgs {
    /// Include built-in and non-USB serial ports such as /dev/ttyS*.
    #[arg(long)]
    pub all: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub struct EmulatorArgs {
    /// Use the rosco emulator instead of a board attached over UART.
    #[arg(short = 'E', long, overrides_with = "hardware")]
    pub emulator: bool,

    /// Use a board attached over UART, overriding defaults.target.
    #[arg(long, overrides_with = "emulator")]
    pub hardware: bool,

    /// Machine to emulate, for example rosco_m68k_010 or rosco_6502.
    #[arg(long)]
    pub machine: Option<String>,

    /// Emulator executable, when it is not on PATH as rosco-emulator.
    #[arg(long, value_name = "PATH")]
    pub emulator_path: Option<PathBuf>,

    /// Directory holding the firmware ROM sets.
    #[arg(long, value_name = "DIR")]
    pub rom_path: Option<PathBuf>,

    /// Image to attach as the emulated SPI SD card.
    #[arg(long, value_name = "IMAGE")]
    pub sd_card: Option<PathBuf>,
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
    fn run_can_target_the_emulator() {
        let cli =
            Cli::try_parse_from(["rosco", "run", "--emulator", "--machine", "rosco_6502"]).unwrap();

        let RoscoCommand::Run(args) = cli.command else {
            panic!("expected run command");
        };
        assert!(args.emulator.emulator);
        assert_eq!(args.emulator.machine.as_deref(), Some("rosco_6502"));
    }

    #[test]
    fn hardware_overrides_a_saved_emulator_default() {
        let cli = Cli::try_parse_from(["rosco", "run", "--hardware"]).unwrap();
        let RoscoCommand::Run(args) = cli.command else {
            panic!("expected run command");
        };
        assert!(args.emulator.hardware);
        assert!(!args.emulator.emulator);
    }

    #[test]
    fn parses_config_set_and_defaults_to_the_user_settings_file() {
        let cli = Cli::try_parse_from(["rosco", "config", "set", "serial.port", "COM3"]).unwrap();
        let RoscoCommand::Config(args) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Set { key, value, scope } = args.command else {
            panic!("expected config set");
        };
        assert_eq!(key, "serial.port");
        assert_eq!(value, "COM3");
        assert!(!scope.local);
    }

    #[test]
    fn config_writes_to_one_file_at_a_time() {
        assert!(
            Cli::try_parse_from([
                "rosco",
                "config",
                "set",
                "serial.port",
                "COM3",
                "--global",
                "--local",
            ])
            .is_err()
        );
    }

    #[test]
    fn upload_can_target_the_emulator() {
        let cli = Cli::try_parse_from(["rosco", "upload", "hello.bin", "--emulator"]).unwrap();
        let RoscoCommand::Upload(args) = cli.command else {
            panic!("expected upload command");
        };
        assert!(args.emulator.emulator);
        assert_eq!(args.file, Some(PathBuf::from("hello.bin")));
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
