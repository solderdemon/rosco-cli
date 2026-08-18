use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const CONFIG_FILE: &str = "rosco.toml";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub project: ProjectConfig,
    pub build: BuildConfig,
    pub serial: SerialConfig,
    pub upload: UploadConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    /// Path relative to the project root. When omitted, software.mk's
    /// `<directory-name>.bin` convention is used.
    pub artifact: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
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
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DockerConfig {
    /// Docker image containing the rosco_m68k toolchain.
    pub image: String,
    /// Optional Docker platform, for example `linux/amd64` on ARM hosts.
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

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SerialConfig {
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

#[derive(Clone, Debug, Deserialize)]
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

impl Config {
    pub fn load(project_root: &Path) -> Result<Self> {
        let path = project_root.join(CONFIG_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }

        let source = fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        toml::from_str(&source).with_context(|| format!("invalid {}", path.display()))
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

    #[test]
    fn defaults_match_rosco_m68k_docker_conventions() {
        let config = Config::default();
        let root = Path::new("/work/hello");
        assert_eq!(config.artifact_path(root).unwrap(), root.join("hello.bin"));
        assert_eq!(config.serial.baud, 38_400);
        assert_eq!(config.build.program, "make");
        assert_eq!(config.build.docker.image, "solderdemon/rosco_m68k:latest");
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
}
