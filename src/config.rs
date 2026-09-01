use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Per-project settings, kept next to the application sources.
pub const CONFIG_FILE: &str = "rosco.toml";
/// Per-user settings, shared by every project on this machine.
pub const GLOBAL_CONFIG_FILE: &str = "config.toml";
/// Overrides the directory holding the per-user settings file.
pub const CONFIG_HOME_ENV: &str = "ROSCO_CONFIG_HOME";
pub const DATA_HOME_ENV: &str = "ROSCO_DATA_HOME";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub project: ProjectConfig,
    pub build: BuildConfig,
    pub serial: SerialConfig,
    pub upload: UploadConfig,
    pub emulator: EmulatorConfig,
    pub defaults: DefaultsConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    /// The board this application is written for.
    pub board: Board,
    /// What it builds: a program the firmware loads, or the firmware itself.
    /// Spelled `type` in the settings file, which is a keyword here.
    #[serde(rename = "type")]
    pub kind: ProjectKind,
    /// Path relative to the project root. When omitted, software.mk's
    /// `<directory-name>.bin` convention is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<PathBuf>,
}

/// What a project builds.
///
/// Almost every project is a `program`: the firmware brings the board up,
/// loads it into RAM and jumps to it, which is why it can print a line
/// without setting up a UART first. A `firmware` project is the other end of
/// that - it is what the machine starts in, with nothing having run before
/// it - so it is burned into ROM rather than sent over a wire, and here it
/// can only be run in the emulator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectKind {
    #[default]
    Program,
    Firmware,
}

impl fmt::Display for ProjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Program => "program",
            Self::Firmware => "firmware",
        })
    }
}

/// Which computer the application is built for.
///
/// Almost everything else follows from it: the toolchain, the machine the
/// emulator runs, and how a binary reaches a board over UART.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
pub enum Board {
    #[default]
    #[serde(rename = "rosco_m68k")]
    #[value(name = "rosco_m68k")]
    RoscoM68k,
    #[serde(rename = "rosco_6502")]
    #[value(name = "rosco_6502")]
    Rosco6502,
}

impl Board {
    /// Toolchain image, when nothing has said otherwise.
    pub fn docker_image(self) -> &'static str {
        match self {
            Self::RoscoM68k => "solderdemon/rosco_m68k:latest",
            Self::Rosco6502 => "solderdemon/rosco_6502:latest",
        }
    }

    /// Machine the emulator runs for this board, when nothing has said
    /// otherwise. The m68k has four of them and they all run the same code.
    pub fn machine(self) -> &'static str {
        match self {
            Self::RoscoM68k => "rosco_m68k_010",
            Self::Rosco6502 => "rosco_6502",
        }
    }

    /// What the emulator calls this board's firmware image. A project that
    /// builds its own has to hand it over under that name.
    pub fn rom_file(self) -> &'static str {
        match self {
            Self::RoscoM68k => "rosco_m68k.rom",
            Self::Rosco6502 => "rosco_6502.rom",
        }
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RoscoM68k => "rosco_m68k",
            Self::Rosco6502 => "rosco_6502",
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildConfig {
    /// Where the compilers come from: a container, or this computer.
    pub toolchain: Toolchain,
    pub program: String,
    pub args: Vec<String>,
    pub clean_args: Vec<String>,
    pub working_directory: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub docker: DockerConfig,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            toolchain: Toolchain::default(),
            program: "make".into(),
            args: vec!["all".into()],
            clean_args: vec!["clean".into()],
            working_directory: PathBuf::from("."),
            environment: BTreeMap::new(),
            docker: DockerConfig::default(),
        }
    }
}

/// Where the compilers that build the application come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Toolchain {
    /// The toolchain image, which needs nothing installed on this computer.
    #[default]
    Docker,
    /// Compilers installed on this computer.
    Host,
}

impl fmt::Display for Toolchain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Docker => "docker",
            Self::Host => "host",
        })
    }
}

/// Docker settings for the toolchain that builds the target application.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DockerConfig {
    /// Docker image containing the rosco_m68k toolchain.
    pub image: String,
    /// Optional Docker platform, for example `linux/amd64` on ARM hosts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            image: "solderdemon/rosco_m68k:latest".into(),
            platform: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SerialConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    pub baud: u32,
    pub read_timeout_ms: u64,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: None,
            baud: 38_400,
            read_timeout_ms: 50,
        }
    }
}

/// Settings for running under the rosco emulator instead of real hardware.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmulatorConfig {
    /// Where the emulator comes from: a container, or this computer.
    pub source: EmulatorSource,
    /// Emulator executable. Left unset, `ROSCO_EMULATOR` is consulted and then
    /// `rosco-emulator` is looked up on `PATH`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<PathBuf>,
    /// Machine to run.
    pub machine: String,
    /// Directory holding the firmware ROM sets, when not the emulator default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rom_path: Option<PathBuf>,
    /// Image to attach as the SPI SD card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sd_card: Option<PathBuf>,
    /// Extra arguments passed straight to the emulator.
    pub args: Vec<String>,
    /// The image used when `source` is `docker`.
    pub docker: EmulatorDockerConfig,
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        Self {
            source: EmulatorSource::default(),
            program: None,
            machine: "rosco_m68k_010".into(),
            rom_path: None,
            sd_card: None,
            args: Vec::new(),
            docker: EmulatorDockerConfig::default(),
        }
    }
}

/// Where the emulator that runs the program comes from.
///
/// The same choice the toolchain has, made separately: a computer with the
/// compilers installed may still have no emulator built, and the other way
/// round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmulatorSource {
    /// A build of the emulator on this computer.
    #[default]
    Host,
    /// The emulator image, which needs nothing installed on this computer.
    Docker,
}

impl fmt::Display for EmulatorSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Host => "host",
            Self::Docker => "docker",
        })
    }
}

/// Docker settings for the emulator, which is a different image from the one
/// holding the toolchain and carries the firmware ROM sets with it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmulatorDockerConfig {
    /// Docker image containing the emulator.
    pub image: String,
    /// Optional Docker platform, for example `linux/amd64` on ARM hosts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

impl Default for EmulatorDockerConfig {
    fn default() -> Self {
        Self {
            image: "solderdemon/rosco-emulator:latest".into(),
            platform: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UploadConfig {
    pub max_retries: u32,
    pub packet_timeout_ms: u64,
    /// Where a rosco_6502 program is loaded and started. The m68k firmware
    /// takes the address from the binary itself and ignores this.
    pub load_address: u32,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            max_retries: 6,
            packet_timeout_ms: 5_000,
            load_address: 0x0800,
        }
    }
}

/// What the commands do when the corresponding flag is not passed.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DefaultsConfig {
    /// Where `upload`, `monitor`, and `run` send the program.
    pub target: Target,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    /// A board attached over UART.
    #[default]
    Hardware,
    /// The rosco emulator.
    Emulator,
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hardware => "hardware",
            Self::Emulator => "emulator",
        })
    }
}

impl Config {
    /// Reads the per-user settings, then the project's, letting the project win.
    pub fn load(project_root: &Path) -> Result<Self> {
        Layers::load(project_root)?.config()
    }

    pub fn build_directory(&self, project_root: &Path) -> PathBuf {
        // The default working directory is `.`, and a path with that still in
        // it reads badly in every message the build prints.
        project_root
            .join(&self.build.working_directory)
            .components()
            .collect()
    }

    pub fn artifact_path(&self, project_root: &Path) -> Result<PathBuf> {
        if let Some(path) = &self.project.artifact {
            return Ok(resolve_from(project_root, path));
        }

        let build_dir = self.build_directory(project_root);
        let name = build_dir
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .context("cannot infer artifact name; set project.artifact in rosco.toml")?;
        // A program is a binary to load; a firmware is a ROM image to burn.
        let extension = match self.project.kind {
            ProjectKind::Program => "bin",
            ProjectKind::Firmware => "rom",
        };
        Ok(build_dir.join(format!("{name}.{extension}")))
    }

    /// The whole configuration as TOML, for listing and reading single keys.
    pub fn to_value(&self) -> Result<toml::Value> {
        toml::Value::try_from(self).context("could not render the settings as TOML")
    }
}

/// Where the per-user settings file lives. `ROSCO_CONFIG_HOME` wins, so a test
/// or a CI job can keep its settings out of the developer's.
pub fn global_config_path() -> Result<PathBuf> {
    if let Some(dir) = non_empty_var(CONFIG_HOME_ENV) {
        return Ok(PathBuf::from(dir).join(GLOBAL_CONFIG_FILE));
    }

    let dir = if cfg!(windows) {
        PathBuf::from(non_empty_var("APPDATA").context(
            "could not locate the settings directory; APPDATA is unset. \
             Set ROSCO_CONFIG_HOME to choose one.",
        )?)
    } else if let Some(base) = non_empty_var("XDG_CONFIG_HOME") {
        PathBuf::from(base)
    } else {
        PathBuf::from(non_empty_var("HOME").context(
            "could not locate the settings directory; HOME is unset. \
             Set ROSCO_CONFIG_HOME to choose one.",
        )?)
        .join(".config")
    };

    Ok(dir.join("rosco").join(GLOBAL_CONFIG_FILE))
}

/// Where things too large for a settings file go, which so far means an
/// emulator built here. `ROSCO_DATA_HOME` wins, as it does for the settings.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = non_empty_var(DATA_HOME_ENV) {
        return Ok(PathBuf::from(dir));
    }

    let dir = if cfg!(windows) {
        PathBuf::from(non_empty_var("LOCALAPPDATA").context(
            "could not locate a directory to install into; LOCALAPPDATA is unset. \
             Set ROSCO_DATA_HOME to choose one.",
        )?)
    } else if let Some(base) = non_empty_var("XDG_DATA_HOME") {
        PathBuf::from(base)
    } else {
        PathBuf::from(non_empty_var("HOME").context(
            "could not locate a directory to install into; HOME is unset. \
             Set ROSCO_DATA_HOME to choose one.",
        )?)
        .join(".local")
        .join("share")
    };

    Ok(dir.join("rosco"))
}

fn non_empty_var(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// Settings that hold nothing until you give them a value, so serializing the
/// configuration does not reveal them. Keep in step with the `Option` fields.
pub const OPTIONAL_SETTINGS: &[&str] = &[
    "build.docker.platform",
    "emulator.docker.platform",
    "emulator.program",
    "emulator.rom_path",
    "emulator.sd_card",
    "project.artifact",
    "serial.port",
];

#[derive(Clone, Debug)]
pub struct Layer {
    pub path: PathBuf,
    pub table: toml::Table,
}

/// Which file a setting came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Default,
    Global,
    Project,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Default => "default",
            Self::Global => "global",
            Self::Project => "project",
        })
    }
}

/// The settings files that apply, lowest precedence first.
#[derive(Clone, Debug, Default)]
pub struct Layers {
    pub global_path: Option<PathBuf>,
    pub project_path: Option<PathBuf>,
    pub global: Option<Layer>,
    pub project: Option<Layer>,
}

impl Layers {
    pub fn load(project_root: &Path) -> Result<Self> {
        // A machine without a home directory still gets to use its projects.
        Self::load_from(global_config_path().ok().as_deref(), Some(project_root))
    }

    pub fn load_from(global_path: Option<&Path>, project_root: Option<&Path>) -> Result<Self> {
        let project_path = project_root.map(|root| root.join(CONFIG_FILE));
        Ok(Self {
            global: global_path.map(read_layer).transpose()?.flatten(),
            project: project_path
                .as_deref()
                .map(read_layer)
                .transpose()?
                .flatten(),
            global_path: global_path.map(Path::to_path_buf),
            project_path,
        })
    }

    /// The files folded together, with the project's values on top.
    pub fn config(&self) -> Result<Config> {
        let mut files = toml::Table::new();
        for layer in [self.global.as_ref(), self.project.as_ref()]
            .into_iter()
            .flatten()
        {
            merge_tables(&mut files, layer.table.clone());
        }

        // The board decides what several other settings default to, so it has
        // to be read before they are resolved. Its defaults go underneath both
        // files, leaving anything written in either of them in charge.
        let mut merged = board_defaults(board_in(&files));
        merge_tables(&mut merged, files);
        toml::Value::Table(merged)
            .try_into()
            .context("could not combine the settings files")
    }

    /// Which file supplies `key`, or `Default` when neither does.
    pub fn origin(&self, key: &str) -> Origin {
        let has = |layer: &Option<Layer>| {
            layer
                .as_ref()
                .is_some_and(|layer| lookup(&layer.table, key).is_some())
        };
        if has(&self.project) {
            Origin::Project
        } else if has(&self.global) {
            Origin::Global
        } else {
            Origin::Default
        }
    }
}

/// What a board brings with it: the image that holds its toolchain and the
/// machine its programs run on.
fn board_defaults(board: Board) -> toml::Table {
    let mut table = toml::Table::new();
    table.insert(
        "build".into(),
        toml::Value::Table(toml::Table::from_iter([(
            "docker".to_string(),
            toml::Value::Table(toml::Table::from_iter([(
                "image".to_string(),
                toml::Value::from(board.docker_image()),
            )])),
        )])),
    );
    table.insert(
        "emulator".into(),
        toml::Value::Table(toml::Table::from_iter([(
            "machine".to_string(),
            toml::Value::from(board.machine()),
        )])),
    );
    table
}

/// The board a settings table names, or the default one when it does not.
/// A misspelled value is left for the schema to complain about by name.
fn board_in(table: &toml::Table) -> Board {
    lookup(table, "project.board")
        .and_then(|value| value.clone().try_into().ok())
        .unwrap_or_default()
}

fn read_layer(path: &Path) -> Result<Option<Layer>> {
    if !path.exists() {
        return Ok(None);
    }

    let source =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    // The schema first: its errors point at the offending line and name the
    // field, which a bare table parse cannot do.
    toml::from_str::<Config>(&source).with_context(|| format!("invalid {}", path.display()))?;
    let table = toml::from_str(&source).with_context(|| format!("invalid {}", path.display()))?;
    Ok(Some(Layer {
        path: path.to_path_buf(),
        table,
    }))
}

/// Tables merge key by key; anything else, arrays included, replaces.
fn merge_tables(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(existing)), toml::Value::Table(incoming)) => {
                merge_tables(existing, incoming);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Follows a dotted key such as `build.docker.image`.
pub fn lookup<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let mut value = table.get(parts.next()?)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    Some(value)
}

pub fn flatten(value: &toml::Value) -> Vec<(String, toml::Value)> {
    let mut leaves = Vec::new();
    walk(String::new(), value, &mut leaves);
    leaves
}

fn walk(prefix: String, value: &toml::Value, leaves: &mut Vec<(String, toml::Value)>) {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                walk(path, child, leaves);
            }
        }
        leaf => leaves.push((prefix, leaf.clone())),
    }
}

pub fn resolve_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn defaults_match_rosco_m68k_docker_conventions() {
        let config = Config::default();
        let root = Path::new("/work/hello");
        assert_eq!(config.artifact_path(root).unwrap(), root.join("hello.bin"));
        assert_eq!(config.serial.baud, 38_400);
        assert_eq!(config.build.program, "make");
        assert_eq!(config.build.docker.image, "solderdemon/rosco_m68k:latest");
        assert_eq!(config.emulator.machine, "rosco_m68k_010");
        assert!(config.emulator.program.is_none());
        assert_eq!(config.defaults.target, Target::Hardware);
        assert_eq!(config.project.board, Board::RoscoM68k);
        assert_eq!(config.project.kind, ProjectKind::Program);
        assert_eq!(config.build.toolchain, Toolchain::Docker);
        assert_eq!(config.upload.load_address, 0x0800);
    }

    #[test]
    fn a_firmware_project_builds_a_rom_image_rather_than_a_binary() {
        let mut config = Config::default();
        config.project.kind = ProjectKind::Firmware;

        let root = Path::new("/work/hello");
        assert_eq!(config.artifact_path(root).unwrap(), root.join("hello.rom"));
    }

    #[test]
    fn a_6502_project_gets_its_own_toolchain_image_and_machine() {
        let project = tempfile::tempdir().unwrap();
        write(
            &project.path().join(CONFIG_FILE),
            "[project]\nboard = \"rosco_6502\"\n",
        );

        let layers = Layers::load_from(None, Some(project.path())).unwrap();
        let config = layers.config().unwrap();

        assert_eq!(config.project.board, Board::Rosco6502);
        assert_eq!(config.build.docker.image, "solderdemon/rosco_6502:latest");
        assert_eq!(config.emulator.machine, "rosco_6502");
        // They came with the board rather than from a file, and say so.
        assert_eq!(layers.origin("build.docker.image"), Origin::Default);
    }

    #[test]
    fn what_a_board_brings_is_still_only_a_default() {
        let project = tempfile::tempdir().unwrap();
        write(
            &project.path().join(CONFIG_FILE),
            "[project]\nboard = \"rosco_6502\"\n\n[emulator]\nmachine = \"rosco_6502_dev\"\n",
        );

        let config = Layers::load_from(None, Some(project.path()))
            .unwrap()
            .config()
            .unwrap();

        assert_eq!(config.emulator.machine, "rosco_6502_dev");
    }

    #[test]
    fn a_board_the_settings_do_not_name_is_left_to_the_schema_to_reject() {
        let project = tempfile::tempdir().unwrap();
        write(
            &project.path().join(CONFIG_FILE),
            "[project]\nboard = \"rosco_z80\"\n",
        );

        let error = Layers::load_from(None, Some(project.path()))
            .unwrap_err()
            .to_string();

        assert!(error.contains("rosco.toml"), "{error}");
    }

    #[test]
    fn parses_emulator_settings() {
        let config: Config = toml::from_str(
            r#"
                [emulator]
                program = "/opt/rosco/rosco"
                machine = "rosco_6502"
                args = ["-nothrottle"]
            "#,
        )
        .unwrap();

        assert_eq!(
            config.emulator.program.as_deref(),
            Some(Path::new("/opt/rosco/rosco"))
        );
        assert_eq!(config.emulator.machine, "rosco_6502");
        assert_eq!(config.emulator.args, ["-nothrottle"]);
    }

    #[test]
    fn parses_custom_cross_platform_builder() {
        let config: Config = toml::from_str(
            r#"
                [project]
                artifact = "out/program.bin"

                [build]
                program = "mingw32-make"
                args = ["release"]
                working_directory = "firmware"

                [build.docker]
                image = "example/rosco-toolchain:v1"
                platform = "linux/amd64"

                [serial]
                port = "COM4"
            "#,
        )
        .unwrap();

        assert_eq!(config.build.program, "mingw32-make");
        assert_eq!(config.build.clean_args, ["clean"]);
        assert_eq!(config.build.docker.platform.as_deref(), Some("linux/amd64"));
        assert_eq!(config.serial.port.as_deref(), Some("COM4"));
    }

    #[test]
    fn rejects_misspelled_fields() {
        let error = toml::from_str::<Config>("[serial]\nbauds = 38400").unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn the_project_overrides_the_user_but_keeps_its_other_settings() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let global = home.path().join(GLOBAL_CONFIG_FILE);
        write(
            &global,
            "[serial]\nport = \"/dev/ttyUSB0\"\nbaud = 9600\n\n[defaults]\ntarget = \"emulator\"\n",
        );
        write(
            &project.path().join(CONFIG_FILE),
            "[serial]\nport = \"COM3\"\n",
        );

        let layers = Layers::load_from(Some(&global), Some(project.path())).unwrap();
        let config = layers.config().unwrap();

        assert_eq!(config.serial.port.as_deref(), Some("COM3"));
        // Untouched by the project file, so the user's value survives.
        assert_eq!(config.serial.baud, 9_600);
        assert_eq!(config.defaults.target, Target::Emulator);
        assert_eq!(config.serial.read_timeout_ms, 50);

        assert_eq!(layers.origin("serial.port"), Origin::Project);
        assert_eq!(layers.origin("serial.baud"), Origin::Global);
        assert_eq!(layers.origin("serial.read_timeout_ms"), Origin::Default);
    }

    #[test]
    fn user_settings_apply_when_the_project_has_no_file() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let global = home.path().join(GLOBAL_CONFIG_FILE);
        write(&global, "[emulator]\nmachine = \"rosco_6502\"\n");

        let config = Layers::load_from(Some(&global), Some(project.path()))
            .unwrap()
            .config()
            .unwrap();

        assert_eq!(config.emulator.machine, "rosco_6502");
    }

    #[test]
    fn a_broken_settings_file_names_itself() {
        let home = tempfile::tempdir().unwrap();
        let global = home.path().join(GLOBAL_CONFIG_FILE);
        write(&global, "[serial]\nbauds = 38400\n");

        let error = Layers::load_from(Some(&global), None).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains(GLOBAL_CONFIG_FILE), "{message}");
        assert!(message.contains("unknown field"), "{message}");
    }

    #[test]
    fn every_optional_setting_is_a_real_one() {
        for key in OPTIONAL_SETTINGS {
            let (section, field) = key.rsplit_once('.').unwrap();
            let source = format!("[{section}]\n{field} = \"x\"\n");
            toml::from_str::<Config>(&source).unwrap_or_else(|error| panic!("{key}: {error}"));
        }
    }

    #[test]
    fn flattening_produces_the_dotted_keys_used_by_the_config_command() {
        let value = Config::default().to_value().unwrap();
        let keys: Vec<_> = flatten(&value).into_iter().map(|(key, _)| key).collect();

        assert!(keys.contains(&"serial.baud".to_string()));
        assert!(keys.contains(&"build.docker.image".to_string()));
        assert!(keys.contains(&"defaults.target".to_string()));
        // Unset optional settings have nothing to show.
        assert!(!keys.contains(&"serial.port".to_string()));
    }
}
