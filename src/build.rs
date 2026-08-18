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
    let mut command = Command::new("docker");
    command
        .args(["run", "--rm"])
        .args(["--user", &user])
        .args(["--env", "HOME=/tmp"])
        .arg("--volume")
        .arg(format!("{}:/workspace", project_root.display()))
        .args(["--workdir", "/workspace"])
        .arg(image)
        .arg("make")
        .args(make_args);

    eprintln!(
        "$ docker run --rm --user {} --env HOME=/tmp --volume {}:/workspace --workdir /workspace {} make {}",
        user,
        project_root.display(),
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
fn host_user() -> String {
    format!("{}:{}", unsafe { libc::geteuid() }, unsafe {
        libc::getegid()
    })
}

#[cfg(not(unix))]
fn host_user() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linker_script_path_matches_the_library_install_target() {
        assert_eq!(
            LIBRARY_LINKER_SCRIPT,
            "libs/build/lib/ld/serial/hugerom_rosco_m68k_program.ld"
        );
    }
}
