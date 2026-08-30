use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use anyhow::{Context, Result, bail};

use crate::config::Config;

const LIBRARY_LINKER_SCRIPT: &str = "libs/build/lib/ld/serial/hugerom_rosco_m68k_program.ld";

#[derive(Debug)]
pub struct BuildOutput {
    pub artifact: PathBuf,
}

pub fn build(project_root: &Path, config: &Config, clean: bool) -> Result<BuildOutput> {
    ensure_docker_available()?;

    if clean {
        run_docker_make(project_root, &config.build.docker.image, &["clean".into()])
            .context("clean command failed")?;
    }

    if !project_root.join(LIBRARY_LINKER_SCRIPT).is_file() {
        run_docker_make(
            project_root,
            &config.build.docker.image,
            &["-C".into(), "libs".into(), "install".into()],
        )
        .context("library installation failed")?;
    }

    let project_name = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .context("cannot determine project directory name")?;
    run_docker_make(
        project_root,
        &config.build.docker.image,
        &[format!("PROGRAM_BASENAME={project_name}")],
    )
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

/// What asking Docker for itself told us.
enum Probe {
    Ready,
    Missing,
    Refused(String),
}

/// Checked before the first container starts, so a missing or stopped Docker
/// is one clear sentence instead of a failed `docker run`.
pub fn ensure_docker_available() -> Result<()> {
    match probe()? {
        Probe::Ready => Ok(()),
        Probe::Missing => bail!("{NO_DOCKER}"),
        Probe::Refused(stderr) => bail!("{}", refusal_message(&stderr)),
    }
}

fn probe() -> Result<Probe> {
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

const NO_DOCKER: &str = "docker is not installed, or is not on PATH.\n  The m68k toolchain runs in a container, so a build needs it: install Docker Desktop or Docker Engine.";

/// Docker is installed but would not answer. Its own words say why far better
/// than a guess would, so they are quoted and only the remedy is added.
fn refusal_message(stderr: &str) -> String {
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

fn run_docker_make(project_root: &Path, image: &str, make_args: &[String]) -> Result<ExitStatus> {
    let user = host_user();
    let volume_source = docker_volume_source(project_root);
    let mut command = Command::new("docker");
    command.args(["run", "--rm"]);
    if let Some(user) = &user {
        command.args(["--user", user]);
    }
    command
        .args(["--env", "HOME=/tmp"])
        .arg("--volume")
        .arg(format!("{volume_source}:/workspace"))
        .args(["--workdir", "/workspace"])
        .arg(image)
        .arg("make")
        .args(make_args);

    let user_args = user
        .as_deref()
        .map(|user| format!(" --user {user}"))
        .unwrap_or_default();
    eprintln!(
        "$ docker run --rm{} --env HOME=/tmp --volume {}:/workspace --workdir /workspace {} make {}",
        user_args,
        volume_source,
        image,
        make_args.join(" ")
    );
    let status = command
        .status()
        .context("could not start docker; install Docker Desktop or Docker Engine")?;
    if !status.success() {
        bail!("Docker make command exited with {status}");
    }
    Ok(status)
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

fn docker_volume_source(path: &Path) -> String {
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
    use super::{
        LIBRARY_LINKER_SCRIPT, docker_volume_source, refusal_message, strip_windows_verbatim_prefix,
    };
    use std::path::Path;

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
}
