use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::config::{Board, Config, ProjectKind, Toolchain};
use crate::toolchain;

const LIBRARY_LINKER_SCRIPT: &str = "libs/build/lib/ld/serial/hugerom_rosco_m68k_program.ld";

/// Where the project is mounted inside the container. Keeping its own
/// directory name means a Makefile that names its output after the directory
/// it builds in - which both boards' conventions do - produces the same file
/// in Docker as it does on this computer.
const CONTAINER_WORKSPACE: &str = "/workspace";

#[derive(Debug)]
pub struct BuildOutput {
    pub artifact: PathBuf,
}

pub fn build(project_root: &Path, config: &Config, clean: bool) -> Result<BuildOutput> {
    let runner = Runner::prepare(project_root, config)?;

    if clean {
        runner
            .run(&config.build.working_directory, &config.build.clean_args)
            .context("clean command failed")?;
    }

    // The m68k libraries are built once and then reused, so this only costs
    // anything on the first build of a project. A firmware has nothing to
    // link against: it is what the libraries would have been calling.
    if config.project.board == Board::RoscoM68k
        && config.project.kind == ProjectKind::Program
        && !project_root.join(LIBRARY_LINKER_SCRIPT).is_file()
    {
        runner
            .run(
                Path::new("."),
                &["-C".to_string(), "libs".to_string(), "install".to_string()],
            )
            .context("library installation failed")?;
    }

    let mut args = config.build.args.clone();
    if config.project.board == Board::RoscoM68k && config.project.kind == ProjectKind::Program {
        // software.mk names its output after the directory it builds in, and
        // says so out loud rather than trusting the mount to be named right.
        args.push(format!(
            "PROGRAM_BASENAME={}",
            build_directory_name(config, project_root)?
        ));
    }
    runner
        .run(&config.build.working_directory, &args)
        .context("build command failed")?;

    let artifact = config.artifact_path(project_root)?;
    if !artifact.is_file() {
        bail!(
            "build succeeded but artifact was not found at {}; set project.artifact in rosco.toml",
            artifact.display()
        );
    }
    Ok(BuildOutput { artifact })
}

fn build_directory_name(config: &Config, project_root: &Path) -> Result<String> {
    config
        .build_directory(project_root)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .context("cannot determine project directory name")
}

/// Runs the build command, either in the toolchain image or on this computer.
struct Runner<'a> {
    project_root: &'a Path,
    config: &'a Config,
    /// The mount point inside the container, when Docker is being used.
    workspace: Option<String>,
}

impl<'a> Runner<'a> {
    /// Checked before the first command runs, so a missing toolchain is one
    /// clear sentence rather than a failure part-way through a build.
    fn prepare(project_root: &'a Path, config: &'a Config) -> Result<Self> {
        let workspace = match config.build.toolchain {
            Toolchain::Docker => {
                ensure_docker_available()?;
                let name = build_directory_name(config, project_root)?;
                Some(format!("{CONTAINER_WORKSPACE}/{name}"))
            }
            Toolchain::Host => {
                ensure_host_toolchain(config.project.board)?;
                None
            }
        };
        Ok(Self {
            project_root,
            config,
            workspace,
        })
    }

    fn run(&self, directory: &Path, args: &[String]) -> Result<()> {
        let (program, arguments) = match &self.workspace {
            Some(workspace) => ("docker", self.docker_arguments(workspace, directory, args)),
            None => (self.config.build.program.as_str(), args.to_vec()),
        };

        eprintln!("$ {program} {}", arguments.join(" "));
        let mut command = Command::new(program);
        command.args(&arguments);
        if self.workspace.is_none() {
            command.current_dir(self.project_root.join(directory));
            command.envs(&self.config.build.environment);
        }

        let status = command
            .status()
            .with_context(|| format!("could not start {program}"))?;
        if !status.success() {
            bail!("{program} exited with {status}");
        }
        Ok(())
    }

    fn docker_arguments(&self, workspace: &str, directory: &Path, args: &[String]) -> Vec<String> {
        let mut arguments = vec!["run".to_string(), "--rm".to_string()];
        if let Some(platform) = &self.config.build.docker.platform {
            arguments.extend(["--platform".to_string(), platform.clone()]);
        }
        // Generated files stay on the host, so they had better belong to the
        // user who asked for them.
        if let Some(user) = host_user() {
            arguments.extend(["--user".to_string(), user]);
        }
        arguments.extend(["--env".to_string(), "HOME=/tmp".to_string()]);
        for (name, value) in &self.config.build.environment {
            arguments.extend(["--env".to_string(), format!("{name}={value}")]);
        }
        arguments.extend([
            "--volume".to_string(),
            format!("{}:{workspace}", docker_volume_source(self.project_root)),
            "--workdir".to_string(),
            container_path(workspace, directory),
            self.config.build.docker.image.clone(),
            self.config.build.program.clone(),
        ]);
        arguments.extend(args.iter().cloned());
        arguments
    }
}

/// The build directory as the container sees it. Its own separators are not
/// the container's, and `.` would leave a path no reader would recognise.
fn container_path(workspace: &str, directory: &Path) -> String {
    let relative = directory
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if relative.is_empty() {
        workspace.to_string()
    } else {
        format!("{workspace}/{relative}")
    }
}

/// What asking Docker for itself told us.
pub enum Probe {
    Ready,
    Missing,
    Refused(String),
}

/// Checked before the first container starts, so a missing or stopped Docker
/// is one clear sentence instead of a failed `docker run`.
pub fn ensure_docker_available() -> Result<()> {
    match docker_probe()? {
        Probe::Ready => Ok(()),
        Probe::Missing => bail!("{NO_DOCKER}"),
        Probe::Refused(stderr) => bail!("{}", refusal_message(&stderr)),
    }
}

/// The same courtesy for a build that runs here: name everything that is
/// missing at once, and say what installs it.
pub fn ensure_host_toolchain(board: Board) -> Result<()> {
    let missing = toolchain::missing(board);
    if missing.is_empty() {
        return Ok(());
    }

    let names: Vec<&str> = missing.iter().map(|tool| tool.command).collect();
    let mut message = format!(
        "the {board} toolchain is not on PATH: {} {} missing.",
        names.join(", "),
        if names.len() == 1 { "is" } else { "are" }
    );
    if let Some(hint) = toolchain::install_hint(board) {
        message.push_str(&format!("\n  Install it with `{hint}`."));
    }
    message.push_str(
        "\n  Or build in the toolchain image instead, with `rosco build --docker` or \
         `rosco config set build.toolchain docker`.",
    );
    bail!("{message}")
}

/// Whether an image is already on this computer, so a pull can be announced
/// rather than happening silently in the middle of something else.
pub fn docker_image_available(image: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether Docker would answer, and when it would not, why. The emulator asks
/// this too, and offers something else instead of insisting on it.
pub fn docker_probe() -> Result<Probe> {
    let output = Command::new("docker")
        .arg("info")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output();

    match output {
        Ok(output) if output.status.success() => Ok(Probe::Ready),
        Ok(output) => Ok(Probe::Refused(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Probe::Missing),
        Err(error) => Err(error).context("could not run docker"),
    }
}

const NO_DOCKER: &str = "docker is not installed, or is not on PATH.\n  The toolchain runs in a container, so a build needs it: install Docker Desktop or Docker Engine, or build with the compilers on this computer with `rosco build --host`.";

/// Docker is installed but would not answer. Its own words say why far better
/// than a guess would, so they are quoted and only the remedy is added.
pub fn refusal_message(stderr: &str) -> String {
    let (headline, remedy) = if stderr.contains("permission denied") {
        (
            "docker is installed but this user cannot reach its daemon.",
            "Add yourself to the docker group with `sudo usermod -aG docker $USER`, then log in again.",
        )
    } else {
        (
            "docker is installed but its daemon is not answering.",
            "Start Docker Desktop, or `sudo systemctl start docker` on Linux.",
        )
    };

    format!("{headline}\n  docker: {}\n  {remedy}", complaint(stderr))
}

fn complaint(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines
        .iter()
        .find(|line| {
            line.starts_with("ERROR:") || line.contains("connect") || line.contains("denied")
        })
        .or_else(|| lines.first())
        .map(|line| line.trim_start_matches("ERROR:").trim().to_string())
        .unwrap_or_else(|| "docker info failed".to_string())
}

#[cfg(unix)]
fn host_user() -> Option<String> {
    Some(format!("{}:{}", unsafe { libc::geteuid() }, unsafe {
        libc::getegid()
    }))
}

#[cfg(not(unix))]
fn host_user() -> Option<String> {
    None
}

/// A path as `docker run --volume` wants to see it. Shared with the emulator,
/// which hands the container a binary to load the same way.
pub fn docker_volume_source(path: &Path) -> String {
    strip_windows_verbatim_prefix(&path.display().to_string())
}

fn strip_windows_verbatim_prefix(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{stripped}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Board;
    use std::path::Path;

    fn runner_for(config: &Config, root: &Path) -> Vec<String> {
        let runner = Runner {
            project_root: root,
            config,
            workspace: Some("/workspace/hello".to_string()),
        };
        runner.docker_arguments(
            "/workspace/hello",
            &config.build.working_directory,
            &["all".to_string()],
        )
    }

    #[test]
    fn a_stopped_daemon_is_explained_in_dockers_own_words() {
        let message = refusal_message(
            "failed to connect to the docker API at unix:///home/me/.docker/desktop/docker.sock; \
             check if the path is correct and if the daemon is running\n",
        );

        assert!(message.contains("daemon is not answering"), "{message}");
        assert!(message.contains("docker.sock"), "{message}");
        assert!(message.contains("systemctl start docker"), "{message}");
    }

    #[test]
    fn a_socket_this_user_cannot_open_suggests_the_group_instead() {
        let message = refusal_message(
            "ERROR: permission denied while trying to connect to the Docker daemon socket\n",
        );

        assert!(message.contains("cannot reach its daemon"), "{message}");
        assert!(message.contains("usermod -aG docker"), "{message}");
        assert!(!message.starts_with("ERROR:"), "{message}");
    }

    #[test]
    fn dockers_noise_around_the_complaint_is_dropped() {
        let message = refusal_message(
            "Client:\n Version: 27.0.3\n\nServer:\nERROR: Cannot connect to the Docker daemon.\nerrors pretty printing info\n",
        );

        assert!(
            message.contains("Cannot connect to the Docker daemon."),
            "{message}"
        );
        assert!(!message.contains("pretty printing"), "{message}");
    }

    #[test]
    fn strips_windows_verbatim_drive_prefix() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\Users\savcu\hello"),
            r"C:\Users\savcu\hello"
        );
    }

    #[test]
    fn strips_windows_verbatim_unc_prefix() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\server\share\hello"),
            r"\\server\share\hello"
        );
    }

    #[test]
    fn leaves_standard_path_unchanged() {
        assert_eq!(
            docker_volume_source(Path::new(r"C:\Users\savcu\hello")),
            r"C:\Users\savcu\hello"
        );
    }

    #[test]
    fn linker_script_path_matches_the_library_install_target() {
        assert_eq!(
            LIBRARY_LINKER_SCRIPT,
            "libs/build/lib/ld/serial/hugerom_rosco_m68k_program.ld"
        );
    }

    #[test]
    fn the_project_keeps_its_directory_name_inside_the_container() {
        let arguments = runner_for(&Config::default(), Path::new("/work/hello"));
        let volume = arguments
            .windows(2)
            .find(|pair| pair[0] == "--volume")
            .expect("the project is mounted");

        assert_eq!(volume[1], "/work/hello:/workspace/hello");
        assert!(arguments.contains(&"/workspace/hello".to_string()));
    }

    #[test]
    fn a_build_directory_is_selected_with_the_containers_own_separator() {
        assert_eq!(
            container_path("/workspace/hello", Path::new(".")),
            "/workspace/hello"
        );
        assert_eq!(
            container_path(
                "/workspace/hello",
                &["firmware", "boot"].iter().collect::<PathBuf>()
            ),
            "/workspace/hello/firmware/boot"
        );
    }

    #[test]
    fn build_variables_reach_the_container() {
        let mut config = Config::default();
        config
            .build
            .environment
            .insert("ROSCO_M68K_DIR".into(), "../rosco_m68k".into());

        let arguments = runner_for(&config, Path::new("/work/hello"));

        assert!(arguments.contains(&"ROSCO_M68K_DIR=../rosco_m68k".to_string()));
    }

    #[test]
    fn an_arm_host_can_ask_for_the_published_platform() {
        let mut config = Config::default();
        config.build.docker.platform = Some("linux/amd64".into());

        let arguments = runner_for(&config, Path::new("/work/hello"));

        assert_eq!(arguments[2], "--platform");
        assert_eq!(arguments[3], "linux/amd64");
    }

    #[test]
    fn a_missing_6502_toolchain_names_what_to_install() {
        // Nothing is asserted about this machine: the message is built from
        // the tool list, and one of the two paths out of it is always offered.
        let message = match ensure_host_toolchain(Board::Rosco6502) {
            Ok(()) => return,
            Err(error) => error.to_string(),
        };

        assert!(message.contains("rosco_6502 toolchain"), "{message}");
        assert!(message.contains("--docker"), "{message}");
    }
}
