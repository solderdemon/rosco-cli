use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail};

use crate::config::Config;

#[derive(Debug)]
pub struct BuildOutput {
    pub artifact: PathBuf,
}

pub fn build(project_root: &Path, config: &Config, clean: bool) -> Result<BuildOutput> {
    let working_directory = config.build_directory(project_root);
    if !working_directory.is_dir() {
        bail!(
            "build directory does not exist: {}",
            working_directory.display()
        );
    }

    if clean {
        run_builder(
            &config.build.program,
            &config.build.clean_args,
            &working_directory,
            &config.build.environment,
        )
        .context("clean command failed")?;
    }

    run_builder(
        &config.build.program,
        &config.build.args,
        &working_directory,
        &config.build.environment,
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

fn run_builder(
    program: &str,
    args: &[String],
    working_directory: &Path,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<ExitStatus> {
    eprintln!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(working_directory)
        .envs(environment)
        .status()
        .with_context(|| format!("could not start `{program}`"))?;
    if !status.success() {
        bail!("`{program}` exited with {status}");
    }
    Ok(status)
}
