use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail};

use crate::config::Config;

const LIBRARY_LINKER_SCRIPT: &str = "libs/build/lib/ld/serial/hugerom_rosco_m68k_program.ld";

#[derive(Debug)]
pub struct BuildOutput {
    pub artifact: PathBuf,
}

pub fn build(project_root: &Path, config: &Config, clean: bool) -> Result<BuildOutput> {
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
    use super::{LIBRARY_LINKER_SCRIPT, docker_volume_source, strip_windows_verbatim_prefix};
    use std::path::Path;

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
