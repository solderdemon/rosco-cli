//! What happens when the emulator is not on this computer yet.
//!
//! Nothing here decides anything on its own: a missing emulator has three
//! answers - its image, a build already here, or one built from source - so
//! the question is asked rather than reported as a failure. The answer is
//! written to the settings, so it is asked once and not on every run.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::build::{self, Probe};
use crate::config::{self, Config, EmulatorSource};
use crate::emulator::{self, DEFAULT_PROGRAM, PROGRAM_ENV, Runner};
use crate::prompt::{self, Choice};
use crate::settings;
use crate::toolchain;

const REPOSITORY: &str = "https://github.com/solderdemon/rosco-emulator";

/// The emulator source tree's own name for its build, which is this CLI's
/// name too; it is only ever used from inside the tree it was built in.
const BUILT_BINARY: &str = "rosco";

/// What runs the machine this time, having made sure it is actually there.
///
/// This is `emulator::resolve_runner` plus the conversation that follows when
/// what it resolved to cannot run.
pub fn ready(
    config: &Config,
    project_root: Option<&Path>,
    from_cli: Option<&Path>,
) -> Result<Runner> {
    let runner = emulator::resolve_runner(config, from_cli)?;
    match &runner {
        Runner::Program(program) => {
            if let Some(found) = locate(program).or_else(|| inside(program)) {
                return Ok(Runner::Program(found));
            }
            // A path on the command line is an answer to this question
            // already, so a wrong one is a typo to report, not a menu.
            if let Some(given) = from_cli {
                bail!(
                    "no emulator at {}; --emulator-path takes the emulator binary, or the \
                     tree it was built in",
                    given.display()
                );
            }
            offer(config, project_root, &missing_program(config, program), 0)
        }
        Runner::Image { .. } => match build::docker_probe()? {
            Probe::Ready => Ok(runner),
            Probe::Missing => offer(config, project_root, NO_DOCKER, 1),
            Probe::Refused(stderr) => {
                offer(config, project_root, &build::refusal_message(&stderr), 1)
            }
        },
    }
}

/// Says what is missing, then asks what to do about it. `start` is the answer
/// worth trying first, which is never the one that just failed.
fn offer(
    config: &Config,
    project_root: Option<&Path>,
    problem: &str,
    start: usize,
) -> Result<Runner> {
    if !prompt::is_interactive() {
        bail!("{problem}\n{}", ADVICE);
    }

    prompt::heading("The emulator is not ready");
    eprintln!("  {problem}\n");
    loop {
        let outcome = match prompt::select("Emulator", CHOICES, start)? {
            0 => use_image(config, project_root)?,
            1 => build_here(config, project_root)?,
            2 => use_existing(config, project_root)?,
            _ => bail!("no emulator chosen.\n{ADVICE}"),
        };
        // Whatever went wrong has said so itself; the menu comes back so the
        // run does not have to be started again to try something else.
        if let Some(runner) = outcome {
            return Ok(runner);
        }
        eprintln!();
    }
}

const CHOICES: &[Choice] = &[
    Choice {
        label: "Docker",
        detail: "run its image; nothing to install",
    },
    Choice {
        label: "Build it here",
        detail: "clone the emulator and build it, which takes a while",
    },
    Choice {
        label: "Existing build",
        detail: "point at an emulator already on this computer",
    },
    Choice {
        label: "Cancel",
        detail: "stop, and print how to set one up later",
    },
];

/// The image needs nothing installed, so this is only ever as far as Docker
/// itself. The image is pulled by the run that follows.
fn use_image(config: &Config, project_root: Option<&Path>) -> Result<Option<Runner>> {
    match build::docker_probe()? {
        Probe::Ready => {}
        Probe::Missing => {
            eprintln!("  {NO_DOCKER}");
            return Ok(None);
        }
        Probe::Refused(stderr) => {
            eprintln!("  {}", build::refusal_message(&stderr));
            return Ok(None);
        }
    }

    remember(
        project_root,
        &[("emulator.source", EmulatorSource::Docker.to_string())],
    );
    Ok(Some(Runner::Image {
        name: config.emulator.docker.image.clone(),
        platform: config.emulator.docker.platform.clone(),
    }))
}

/// Clones the emulator and builds it, which is a MAME build: long, and with
/// dependencies of its own. Both are said out loud before anything starts.
fn build_here(config: &Config, project_root: Option<&Path>) -> Result<Option<Runner>> {
    if let Some(missing) = missing_build_tools() {
        eprintln!("  {missing}");
        return Ok(None);
    }

    eprintln!(
        "  The emulator is MAME with everything but rosco removed: a large checkout and\n  \
         a build that takes tens of minutes, and it wants SDL2 and the rest of MAME's\n  \
         build dependencies. Tagged releases carry prebuilt Linux binaries instead.\n  \
         {REPOSITORY}#building"
    );
    let default = default_source_dir()?;
    let answer = prompt::text("Build in", &default.to_string_lossy())?;
    let tree = expand_home(Path::new(answer.trim()));
    if prompt::select("Build it now", CONFIRM, 0)? != 0 {
        return Ok(None);
    }

    if let Err(error) = fetch(&tree).and_then(|()| compile(&tree)) {
        eprintln!("  {error:#}");
        return Ok(None);
    }

    let program = tree.join(BUILT_BINARY);
    if !program.is_file() {
        eprintln!(
            "  the build finished but left no {BUILT_BINARY} in {}",
            tree.display()
        );
        return Ok(None);
    }
    eprintln!("Built {}", program.display());
    remember_program(config, project_root, &program);
    Ok(Some(Runner::Program(program)))
}

fn use_existing(config: &Config, project_root: Option<&Path>) -> Result<Option<Runner>> {
    let suggestion = std::env::var(PROGRAM_ENV).unwrap_or_else(|_| DEFAULT_PROGRAM.to_string());
    let answer = prompt::text("Emulator path", &suggestion)?;
    let given = expand_home(Path::new(answer.trim()));

    let Some(program) = locate(&given).or_else(|| inside(&given)) else {
        eprintln!("  there is nothing to run at {}", given.display());
        return Ok(None);
    };
    if std::env::current_exe().is_ok_and(|current| emulator::same_file(&program, &current)) {
        eprintln!(
            "  {} is this CLI, not the emulator; in its own tree the emulator binary has \
             the same name",
            program.display()
        );
        return Ok(None);
    }

    remember_program(config, project_root, &program);
    Ok(Some(Runner::Program(program)))
}

const CONFIRM: &[Choice] = &[
    Choice {
        label: "Build",
        detail: "clone and compile now",
    },
    Choice {
        label: "Back",
        detail: "return to the other ways of getting one",
    },
];

fn fetch(tree: &Path) -> Result<()> {
    if tree.join(".git").is_dir() {
        eprintln!("Using the checkout already in {}", tree.display());
        return Ok(());
    }
    if tree.exists() {
        bail!(
            "{} exists and is not a checkout of the emulator",
            tree.display()
        );
    }

    eprintln!("Cloning {REPOSITORY} into {}", tree.display());
    run(
        Command::new("git")
            .arg("clone")
            .arg("--depth")
            .arg("1")
            .arg(REPOSITORY)
            .arg(tree),
        "git clone",
    )
}

fn compile(tree: &Path) -> Result<()> {
    let jobs = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    eprintln!("Building the emulator in {}", tree.display());
    // The Qt debugger is the one dependency worth dropping: nothing here ever
    // opens a window, and without it the build needs no Qt installed at all.
    // REGENIE=1 is what makes the makefile notice that choice.
    run(
        Command::new("make")
            .current_dir(tree)
            .arg(format!("-j{jobs}"))
            .arg("REGENIE=1")
            .arg("USE_QTDEBUG=0"),
        "make",
    )
}

/// Runs a step of the installation with its output left on the terminal:
/// these take long enough that silence would look like a hang.
fn run(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("could not run {what}"))?;
    if !status.success() {
        bail!("{what} exited with {status}");
    }
    Ok(())
}

/// Everything the emulator's build needs that is not here, as one sentence.
fn missing_build_tools() -> Option<String> {
    let mut missing: Vec<&str> = ["git", "make", "python3", "c++"]
        .into_iter()
        .filter(|tool| !toolchain::available(tool))
        .collect();
    // SDL is a library rather than a program, so pkg-config is what can be
    // asked about it - and only when it is installed itself.
    if toolchain::available("pkg-config") && !pkg_config_has("sdl2") {
        missing.push("libsdl2-dev");
    }
    if missing.is_empty() {
        return None;
    }

    let mut message = format!(
        "the emulator's build needs {}, which {} not here.",
        missing.join(", "),
        if missing.len() == 1 { "is" } else { "are" }
    );
    if cfg!(target_os = "linux") {
        message.push_str(
            "\n  On Debian or Ubuntu: `sudo apt install build-essential git python3 pkg-config \
             libsdl2-dev libsdl2-ttf-dev libfontconfig-dev libx11-dev libxinerama-dev \
             libxext-dev libxi-dev libgl-dev libasound2-dev libpulse-dev`.",
        );
    }
    message.push_str(&format!("\n  {REPOSITORY}#building lists them all."));
    Some(message)
}

fn pkg_config_has(library: &str) -> bool {
    Command::new("pkg-config")
        .args(["--exists", library])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Saves the answer, and says where it went. Failing to write it costs the
/// next run the same question and nothing more, so it is not an error.
fn remember(project_root: Option<&Path>, settings: &[(&str, String)]) {
    let keys: Vec<&str> = settings.iter().map(|(key, _)| *key).collect();
    match settings::remember(project_root, settings) {
        Ok(path) => eprintln!("Saved {} to {}", keys.join(" and "), path.display()),
        Err(error) => eprintln!("(this could not be saved for next time: {error:#})"),
    }
}

fn remember_program(config: &Config, project_root: Option<&Path>, program: &Path) {
    remember(project_root, &program_settings(config, program));
}

/// A path alone would be read past by settings that send every run to the
/// image, so where that is what they say, the source is recorded with it.
fn program_settings(config: &Config, program: &Path) -> Vec<(&'static str, String)> {
    let mut settings = vec![("emulator.program", program.display().to_string())];
    if config.emulator.source == EmulatorSource::Docker {
        settings.push(("emulator.source", EmulatorSource::Host.to_string()));
    }
    settings
}

/// Why there is nothing to run, named after whatever was consulted last.
fn missing_program(config: &Config, program: &Path) -> String {
    if config.emulator.program.is_some() {
        format!(
            "emulator.program points at {}, and there is nothing there.",
            program.display()
        )
    } else if std::env::var_os(PROGRAM_ENV).is_some() {
        format!(
            "{PROGRAM_ENV} points at {}, and there is nothing there.",
            program.display()
        )
    } else {
        format!("{DEFAULT_PROGRAM} is not on PATH, and no emulator is saved in the settings.")
    }
}

const NO_DOCKER: &str = "docker is not installed, or is not on PATH.";

const ADVICE: &str = "  \
     Its image needs nothing installed: `rosco config set emulator.source docker`.\n  \
     Or build it from https://github.com/solderdemon/rosco-emulator and say where it went:\n    \
     `rosco config set emulator.program <path>/rosco`.\n  \
     --emulator-path and ROSCO_EMULATOR point at one for a single run.";

/// Where a build goes when the user has no opinion: beside the settings
/// rather than in whatever directory the command happened to run in.
fn default_source_dir() -> Result<PathBuf> {
    Ok(config::data_dir()?.join("rosco-emulator"))
}

/// `~` as the shell would have expanded it, since nothing here goes through
/// one and a leading tilde is the natural thing to type.
fn expand_home(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match home() {
        Some(home) => home.join(rest),
        None => path.to_path_buf(),
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The emulator binary this name refers to, if it is there at all: a name on
/// its own is looked up on `PATH`, anything else is taken as a path.
pub fn locate(program: &Path) -> Option<PathBuf> {
    let bare = program
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty());
    if !bare {
        return runnable(&expand_home(program));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|directory| !directory.as_os_str().is_empty())
        .find_map(|directory| runnable(&directory.join(program)))
}

/// The emulator inside a directory, for a path that names the tree it was
/// built in rather than the binary.
fn inside(directory: &Path) -> Option<PathBuf> {
    if !directory.is_dir() {
        return None;
    }
    [BUILT_BINARY, DEFAULT_PROGRAM]
        .into_iter()
        .find_map(|name| runnable(&directory.join(name)))
}

fn runnable(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    // Windows leaves the extension off the name it is called by.
    if std::env::consts::EXE_SUFFIX.is_empty() {
        return None;
    }
    let mut name = path.as_os_str().to_os_string();
    name.push(std::env::consts::EXE_SUFFIX);
    let candidate = PathBuf::from(name);
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_taken_as_one_even_when_nothing_is_there() {
        assert_eq!(locate(Path::new("/definitely/not/here/rosco")), None);
    }

    #[test]
    fn a_file_that_is_there_is_the_one_that_would_run() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("rosco-emulator");
        std::fs::write(&program, "").unwrap();

        assert_eq!(locate(&program).as_deref(), Some(program.as_path()));
        assert_eq!(locate(&directory.path().join("nothing")), None);
    }

    #[test]
    fn a_leading_tilde_becomes_the_home_directory() {
        let Some(home) = home() else { return };
        assert_eq!(
            expand_home(Path::new("~/emu/rosco")),
            home.join("emu/rosco")
        );
    }

    #[test]
    fn a_path_without_a_tilde_is_left_alone() {
        assert_eq!(
            expand_home(Path::new("/opt/rosco/rosco")),
            PathBuf::from("/opt/rosco/rosco")
        );
    }

    #[test]
    fn a_tree_stands_in_for_the_binary_built_in_it() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join(BUILT_BINARY);
        std::fs::write(&program, "").unwrap();

        assert_eq!(inside(directory.path()).as_deref(), Some(program.as_path()));
        assert_eq!(inside(&program), None);
    }

    #[test]
    fn a_build_chosen_over_the_image_stops_the_image_being_used() {
        let mut config = Config::default();
        config.emulator.source = EmulatorSource::Docker;

        let settings = program_settings(&config, Path::new("/opt/rosco/rosco"));

        assert_eq!(
            settings,
            [
                ("emulator.program", "/opt/rosco/rosco".to_string()),
                ("emulator.source", "host".to_string()),
            ]
        );
    }

    #[test]
    fn a_build_where_one_was_already_expected_records_only_the_path() {
        let settings = program_settings(&Config::default(), Path::new("/opt/rosco/rosco"));
        assert_eq!(settings.len(), 1);
    }

    #[test]
    fn the_missing_emulator_names_what_pointed_at_it() {
        let mut config = Config::default();
        config.emulator.program = Some("/opt/rosco/rosco".into());
        let message = missing_program(&config, Path::new("/opt/rosco/rosco"));
        assert!(
            message.contains("emulator.program points at /opt/rosco/rosco"),
            "{message}"
        );
    }
}
