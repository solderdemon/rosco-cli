use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::{Board, ProjectKind, Target};

/// Build, upload, and monitor programs for rosco_m68k and rosco_6502.
#[derive(Debug, Parser)]
#[command(name = "rosco", version, propagate_version = true)]
pub struct Cli {
    /// Application directory.
    #[arg(short = 'C', long, global = true, default_value = ".")]
    pub project: PathBuf,

    #[command(subcommand)]
    pub command: RoscoCommand,
}

#[derive(Debug, Subcommand)]
pub enum RoscoCommand {
    /// Create a new rosco_m68k or rosco_6502 project.
    Init(InitArgs),
    /// Compile the current application.
    Build(BuildArgs),
    /// Upload a binary through UART.
    Upload(UploadArgs),
    /// Open an interactive UART session.
    Monitor(MonitorArgs),
    /// Build, upload, and open an interactive UART session.
    Run(RunArgs),
    /// List USB serial ports that may be connected to a board.
    Ports(PortsArgs),
    /// Read and write saved settings.
    Config(ConfigArgs),
    /// Check Docker, the toolchain, the emulator, and UART setup.
    Doctor,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Directory to create. It must not already exist.
    ///
    /// The older form, a language before the name, is still understood:
    /// `rosco init asm hello` and `rosco init hello --language asm` are the
    /// same request.
    #[arg(value_name = "NAME", num_args = 0..=2)]
    pub name: Vec<String>,

    /// Board the project is written for.
    #[arg(short = 'b', long)]
    pub board: Option<Board>,

    /// Project language.
    #[arg(short = 'l', long)]
    pub language: Option<ProjectLanguage>,

    /// What the project builds: a program for the firmware to load, or the
    /// firmware the machine starts in.
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    pub kind: Option<ProjectKind>,

    /// Where run and upload send the program without a flag.
    #[arg(long, value_name = "TARGET")]
    pub target: Option<Target>,

    /// Run the emulator from its image, so nothing has to be installed.
    #[arg(long, conflicts_with = "emulator_path")]
    pub emulator_docker: bool,

    /// The emulator this project runs on, recorded so no later command needs
    /// --emulator-path.
    #[arg(long, value_name = "PATH")]
    pub emulator_path: Option<PathBuf>,

    #[command(flatten)]
    pub toolchain: ToolchainArgs,

    /// Take the default for anything not given instead of asking for it.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ProjectLanguage {
    C,
    Asm,
}

impl ProjectLanguage {
    pub fn label(self) -> &'static str {
        match self {
            Self::C => "c",
            Self::Asm => "asm",
        }
    }
}

#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Run the configured clean command before building.
    #[arg(long)]
    pub clean: bool,

    #[command(flatten)]
    pub toolchain: ToolchainArgs,
}

/// Where this run takes its compilers from, when it is not what
/// `build.toolchain` says.
#[derive(Clone, Copy, Debug, Default, Args)]
pub struct ToolchainArgs {
    /// Build in the toolchain image.
    #[arg(long, overrides_with = "host")]
    pub docker: bool,

    /// Build with the compilers installed on this computer.
    #[arg(long, overrides_with = "docker")]
    pub host: bool,
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
    pub toolchain: ToolchainArgs,

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
    fn init_still_takes_the_language_before_the_name() {
        let cli = Cli::try_parse_from(["rosco", "init", "asm", "hello"]).unwrap();
        let RoscoCommand::Init(args) = cli.command else {
            panic!("expected init command");
        };
        assert_eq!(args.name, ["asm", "hello"]);
        assert_eq!(args.language, None);
    }

    #[test]
    fn init_asks_for_nothing_it_was_told_on_the_command_line() {
        let cli = Cli::try_parse_from([
            "rosco",
            "init",
            "hello",
            "--board",
            "rosco_6502",
            "--language",
            "asm",
            "--host",
            "--target",
            "emulator",
        ])
        .unwrap();

        let RoscoCommand::Init(args) = cli.command else {
            panic!("expected init command");
        };
        assert_eq!(args.name, ["hello"]);
        assert_eq!(args.board, Some(Board::Rosco6502));
        assert_eq!(args.language, Some(ProjectLanguage::Asm));
        assert_eq!(args.target, Some(Target::Emulator));
        assert!(args.toolchain.host);
    }

    #[test]
    fn init_takes_the_emulator_from_the_image_or_from_a_path() {
        let cli = Cli::try_parse_from(["rosco", "init", "hello", "--emulator-docker"]).unwrap();
        let RoscoCommand::Init(args) = cli.command else {
            panic!("expected init command");
        };
        assert!(args.emulator_docker);
        assert!(args.emulator_path.is_none());

        let cli = Cli::try_parse_from([
            "rosco",
            "init",
            "hello",
            "--emulator-path",
            "/opt/rosco/rosco",
        ])
        .unwrap();
        let RoscoCommand::Init(args) = cli.command else {
            panic!("expected init command");
        };
        assert_eq!(args.emulator_path, Some(PathBuf::from("/opt/rosco/rosco")));
    }

    #[test]
    fn init_can_be_asked_for_a_firmware_project() {
        let cli = Cli::try_parse_from(["rosco", "init", "hello", "--type", "firmware"]).unwrap();
        let RoscoCommand::Init(args) = cli.command else {
            panic!("expected init command");
        };
        assert_eq!(args.kind, Some(ProjectKind::Firmware));
    }

    #[test]
    fn init_takes_the_emulator_from_one_place_at_a_time() {
        assert!(
            Cli::try_parse_from([
                "rosco",
                "init",
                "hello",
                "--emulator-docker",
                "--emulator-path",
                "/opt/rosco/rosco",
            ])
            .is_err()
        );
    }

    #[test]
    fn init_with_no_arguments_is_a_conversation_rather_than_an_error() {
        let cli = Cli::try_parse_from(["rosco", "init"]).unwrap();
        let RoscoCommand::Init(args) = cli.command else {
            panic!("expected init command");
        };
        assert!(args.name.is_empty());
        assert!(args.board.is_none());
    }

    #[test]
    fn a_build_can_choose_its_compilers_for_one_run() {
        let cli = Cli::try_parse_from(["rosco", "build", "--host"]).unwrap();
        let RoscoCommand::Build(args) = cli.command else {
            panic!("expected build command");
        };
        assert!(args.toolchain.host);
        assert!(!args.toolchain.docker);

        // The last flag wins, rather than the pair being an error.
        let cli = Cli::try_parse_from(["rosco", "run", "--host", "--docker"]).unwrap();
        let RoscoCommand::Run(args) = cli.command else {
            panic!("expected run command");
        };
        assert!(args.toolchain.docker);
        assert!(!args.toolchain.host);
    }
}
