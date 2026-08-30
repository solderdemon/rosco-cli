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
    /// Path relative to the project root. When omitted, software.mk's
    /// `<directory-name>.bin` convention is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildConfig {
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
            program: "make".into(),
            args: vec!["all".into()],
            clean_args: vec!["clean".into()],
            working_directory: PathBuf::from("."),
            environment: BTreeMap::new(),
            docker: DockerConfig::default(),
        }
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
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        Self {
            program: None,
            machine: "rosco_m68k_010".into(),
            rom_path: None,
            sd_card: None,
            args: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UploadConfig {
    pub max_retries: u32,
    pub packet_timeout_ms: u64,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            max_retries: 6,
            packet_timeout_ms: 5_000,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
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
        project_root.join(&self.build.working_directory)
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
        Ok(build_dir.join(format!("{name}.bin")))
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

fn non_empty_var(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// Settings that hold nothing until you give them a value, so serializing the
/// configuration does not reveal them. Keep in step with the `Option` fields.
pub const OPTIONAL_SETTINGS: &[&str] = &[
    "build.docker.platform",
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
        let mut merged = toml::Table::new();
        for layer in [self.global.as_ref(), self.project.as_ref()]
            .into_iter()
            .flatten()
        {
            merge_tables(&mut merged, layer.table.clone());
        }
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
            "[serial]\nport = \"/dev/ttyUSB0\"\nbaud = 115200\n\n[defaults]\ntarget = \"emulator\"\n",
        );
        write(
            &project.path().join(CONFIG_FILE),
            "[serial]\nport = \"COM3\"\n",
        );

        let layers = Layers::load_from(Some(&global), Some(project.path())).unwrap();
        let config = layers.config().unwrap();

        assert_eq!(config.serial.port.as_deref(), Some("COM3"));
        // Untouched by the project file, so the user's value survives.
        assert_eq!(config.serial.baud, 115_200);
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
