use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use include_dir::{Dir, DirEntry, include_dir};
use serde::{Deserialize, Serialize};

use crate::cli::ProjectLanguage;
use crate::config::{self, Board, CONFIG_FILE, EmulatorSource, ProjectKind, Target, Toolchain};

static M68K_COMMON: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/rosco_m68k/common");
static M68K_C: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/rosco_m68k/c");
static M68K_ASM: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/rosco_m68k/asm");
static MOS6502_COMMON: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/rosco_6502/common");
static MOS6502_C: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/rosco_6502/c");
static MOS6502_ASM: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/rosco_6502/asm");
static M68K_FIRMWARE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/rosco_m68k/firmware");
static MOS6502_FIRMWARE: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/templates/rosco_6502/firmware");

/// Everything `init` was told or asked about the project it is creating.
#[derive(Clone, Debug)]
pub struct ProjectPlan {
    pub board: Board,
    pub kind: ProjectKind,
    pub language: ProjectLanguage,
    pub toolchain: Toolchain,
    pub target: Target,
    pub emulator: EmulatorSetup,
}

/// How the project reaches the emulator.
///
/// The point of the last two is that the answer is given once here instead of
/// on the end of every command that runs something.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum EmulatorSetup {
    /// Nothing is written down: `--emulator-path`, then `ROSCO_EMULATOR`, then
    /// `rosco-emulator` on `PATH` decides each run.
    #[default]
    EachRun,
    /// The emulator image, which needs nothing installed on this computer.
    Docker,
    /// A build of the emulator on this computer, recorded in the project.
    Program(PathBuf),
}

impl EmulatorSetup {
    /// One phrase for the summaries `init` prints and offers back. They sit
    /// on one line of a terminal, so they are as short as they can be.
    fn summary(&self) -> String {
        match self {
            Self::EachRun => "no emulator saved".to_string(),
            Self::Docker => "docker emulator".to_string(),
            Self::Program(path) => format!("emulator {}", path.display()),
        }
    }
}

impl ProjectPlan {
    /// One line naming every choice, for the confirmation `init` prints when
    /// it is done.
    pub fn describe(&self) -> String {
        describe(
            self.board,
            self.kind,
            self.language,
            self.toolchain,
            self.target,
            &self.emulator,
        )
    }
}

/// A firmware project leaves out what it never got to choose: it is assembly,
/// and the emulator is the only thing here that can run it.
fn describe(
    board: Board,
    kind: ProjectKind,
    language: ProjectLanguage,
    toolchain: Toolchain,
    target: Target,
    emulator: &EmulatorSetup,
) -> String {
    let what = match kind {
        ProjectKind::Program => format!("{board}/{}, {target}", language.label()),
        ProjectKind::Firmware => format!("{board} firmware"),
    };
    format!("{what}, {toolchain} build, {}", emulator.summary())
}

pub fn create(plan: &ProjectPlan, destination: &Path) -> Result<()> {
    if fs::symlink_metadata(destination).is_ok() {
        bail!("destination already exists: {}", destination.display());
    }

    for template in templates(plan) {
        copy_dir(template, destination)?;
    }

    let settings = destination.join(CONFIG_FILE);
    fs::write(&settings, project_settings(plan))
        .with_context(|| format!("could not write {}", settings.display()))?;
    Ok(())
}

/// The template directories a project is made of, copied in order.
///
/// A firmware project shares nothing with a program one: it has no C library
/// to link against and no linker configuration that puts it in RAM, so it
/// gets a directory of its own rather than the shared one plus a language.
fn templates(plan: &ProjectPlan) -> Vec<&'static Dir<'static>> {
    match (plan.kind, plan.board, plan.language) {
        (ProjectKind::Firmware, Board::RoscoM68k, _) => vec![&M68K_FIRMWARE],
        (ProjectKind::Firmware, Board::Rosco6502, _) => vec![&MOS6502_FIRMWARE],
        (ProjectKind::Program, Board::RoscoM68k, ProjectLanguage::C) => {
            vec![&M68K_COMMON, &M68K_C]
        }
        (ProjectKind::Program, Board::RoscoM68k, ProjectLanguage::Asm) => {
            vec![&M68K_COMMON, &M68K_ASM]
        }
        (ProjectKind::Program, Board::Rosco6502, ProjectLanguage::C) => {
            vec![&MOS6502_COMMON, &MOS6502_C]
        }
        (ProjectKind::Program, Board::Rosco6502, ProjectLanguage::Asm) => {
            vec![&MOS6502_COMMON, &MOS6502_ASM]
        }
    }
}

/// The project's `rosco.toml`.
///
/// The board is written whatever it is, because it is a fact about the
/// application rather than a preference. The rest is only written when it
/// differs from the default: a repeated default in here would quietly
/// override whatever the same user saved for themselves.
fn project_settings(plan: &ProjectPlan) -> String {
    let mut settings = String::from(
        "# Settings for this project. `rosco config list` shows every setting,\n\
         # its value, and which file decided it.\n\n[project]\n",
    );
    let _ = writeln!(settings, "board = \"{}\"", plan.board);
    if plan.kind != ProjectKind::default() {
        let _ = write!(
            settings,
            "# This builds the machine's firmware rather than a program for it\n\
             # to load, so it is a ROM image and only the emulator can run it.\n\
             type = \"{}\"\n",
            plan.kind
        );
    }

    if plan.toolchain != Toolchain::default() {
        let _ = write!(
            settings,
            "\n[build]\n# The compilers on this computer, rather than the toolchain image.\n\
             toolchain = \"{}\"\n",
            plan.toolchain
        );
    }
    match &plan.emulator {
        // Left out on purpose: with nothing here, ROSCO_EMULATOR and PATH
        // still decide, and a saved user setting is not overridden.
        EmulatorSetup::EachRun => {}
        EmulatorSetup::Docker => {
            let _ = write!(
                settings,
                "\n[emulator]\n# The emulator image, rather than a build of it on this computer.\n\
                 source = \"{}\"\n",
                EmulatorSource::Docker
            );
        }
        EmulatorSetup::Program(path) => {
            let _ = write!(
                settings,
                "\n[emulator]\n# The emulator this project runs on, so no command needs \
                 --emulator-path.\nprogram = {}\n",
                toml_string(&path.display().to_string())
            );
        }
    }
    if plan.target != Target::default() {
        let _ = write!(
            settings,
            "\n[defaults]\n# Where run and upload send the program without a flag.\n\
             target = \"{}\"\n",
            plan.target
        );
    }
    settings
}

/// A path as TOML spells it, backslashes and all, which matters on Windows.
fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

/// What `rosco init` was told last time.
///
/// Kept beside the per-user settings, because it describes the person rather
/// than any one project. It is a memory of the last answers and nothing else:
/// no command reads it to decide anything, so a file that has gone missing or
/// stale only costs the offer to reuse it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Remembered {
    pub board: Board,
    /// Absent from files written before firmware projects existed, and
    /// spelled the way the settings file spells it.
    #[serde(default, rename = "type")]
    pub kind: ProjectKind,
    pub language: ProjectLanguage,
    pub toolchain: Toolchain,
    pub target: Target,
    emulator: EmulatorChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    emulator_program: Option<PathBuf>,
}

/// Which of the three answers about the emulator was given. The path that
/// goes with `program` is a field of its own, so the file stays flat enough
/// to edit by hand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum EmulatorChoice {
    #[default]
    EachRun,
    Docker,
    Program,
}

#[derive(Debug, Deserialize, Serialize)]
struct RememberedFile {
    last: Remembered,
}

const REMEMBERED_FILE: &str = "init.toml";

const REMEMBERED_HEADER: &str = "\
# The answers `rosco init` was given last time, which it offers back as\n\
# \"Previous setup\". Delete this file to be asked from the defaults again.\n\n";

impl Remembered {
    pub fn of(plan: &ProjectPlan) -> Self {
        let (emulator, emulator_program) = match &plan.emulator {
            EmulatorSetup::EachRun => (EmulatorChoice::EachRun, None),
            EmulatorSetup::Docker => (EmulatorChoice::Docker, None),
            EmulatorSetup::Program(path) => (EmulatorChoice::Program, Some(path.clone())),
        };
        Self {
            board: plan.board,
            kind: plan.kind,
            language: plan.language,
            toolchain: plan.toolchain,
            target: plan.target,
            emulator,
            emulator_program,
        }
    }

    pub fn emulator(&self) -> EmulatorSetup {
        match (self.emulator, &self.emulator_program) {
            (EmulatorChoice::Docker, _) => EmulatorSetup::Docker,
            (EmulatorChoice::Program, Some(path)) => EmulatorSetup::Program(path.clone()),
            // A `program` answer with no path left is no answer at all.
            _ => EmulatorSetup::EachRun,
        }
    }

    /// The one line that goes under "Previous setup", so the choice can be
    /// made without having to remember what was chosen.
    pub fn summary(&self) -> String {
        describe(
            self.board,
            self.kind,
            self.language,
            self.toolchain,
            self.target,
            &self.emulator(),
        )
    }

    /// The last answers, or nothing at all: this is a convenience, so a file
    /// that cannot be read is the same as one that was never written.
    pub fn load() -> Option<Self> {
        let path = remembered_path().ok()?;
        let source = fs::read_to_string(path).ok()?;
        Some(toml::from_str::<RememberedFile>(&source).ok()?.last)
    }

    pub fn save(&self) -> Result<()> {
        let path = remembered_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let file = RememberedFile { last: self.clone() };
        let body = toml::to_string_pretty(&file).context("could not render the last answers")?;
        fs::write(&path, format!("{REMEMBERED_HEADER}{body}"))
            .with_context(|| format!("could not write {}", path.display()))
    }
}

pub fn remembered_path() -> Result<PathBuf> {
    Ok(config::global_config_path()?.with_file_name(REMEMBERED_FILE))
}

fn copy_dir(template: &Dir<'_>, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("could not create {}", destination.display()))?;
    for entry in template.entries() {
        copy_entry(entry, destination)?;
    }
    Ok(())
}

fn copy_entry(entry: &DirEntry<'_>, destination: &Path) -> Result<()> {
    match entry {
        DirEntry::Dir(dir) => {
            let path = destination.join(dir.path());
            fs::create_dir_all(&path)
                .with_context(|| format!("could not create {}", path.display()))?;
            for child in dir.entries() {
                copy_entry(child, destination)?;
            }
        }
        DirEntry::File(file) => {
            let path = destination.join(file.path());
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("could not create {}", parent.display()))?;
            }
            fs::write(&path, file.contents())
                .with_context(|| format!("could not write {}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(board: Board, language: ProjectLanguage) -> ProjectPlan {
        ProjectPlan {
            board,
            kind: ProjectKind::Program,
            language,
            toolchain: Toolchain::Docker,
            target: Target::Hardware,
            emulator: EmulatorSetup::EachRun,
        }
    }

    fn create_in(plan: &ProjectPlan, name: &str) -> tempfile::TempDir {
        let parent = tempfile::tempdir().unwrap();
        create(plan, &parent.path().join(name)).unwrap();
        parent
    }

    #[test]
    fn a_68k_project_gets_the_shared_libraries_and_its_language() {
        let parent = create_in(&plan(Board::RoscoM68k, ProjectLanguage::C), "hello");
        let project = parent.path().join("hello");

        assert!(project.join("software.mk").is_file());
        assert!(project.join("libs/Makefile").is_file());
        assert!(project.join("kmain.c").is_file());
        assert!(project.join("Makefile").is_file());
    }

    #[test]
    fn a_6502_project_gets_the_linker_configuration_and_the_hardware_defines() {
        let parent = create_in(&plan(Board::Rosco6502, ProjectLanguage::Asm), "hello");
        let project = parent.path().join("hello");

        assert!(project.join("rosco_6502.cfg").is_file());
        assert!(project.join("inc/defines.inc").is_file());
        assert!(project.join("main.s").is_file());
        assert!(project.join("console.s").is_file());
        assert!(!project.join("main.c").exists());
    }

    #[test]
    fn a_firmware_project_gets_the_firmware_template_and_nothing_else() {
        let parent = create_in(
            &ProjectPlan {
                kind: ProjectKind::Firmware,
                ..plan(Board::Rosco6502, ProjectLanguage::Asm)
            },
            "hello",
        );
        let project = parent.path().join("hello");

        assert!(project.join("firmware.s").is_file());
        assert!(project.join("rosco_6502.cfg").is_file());
        // Nothing a program links against, because there is nothing under it
        // to link against.
        assert!(!project.join("main.s").exists());
        assert!(!project.join("inc/defines.inc").exists());
    }

    #[test]
    fn an_m68k_firmware_project_is_not_given_the_libraries() {
        let parent = create_in(
            &ProjectPlan {
                kind: ProjectKind::Firmware,
                ..plan(Board::RoscoM68k, ProjectLanguage::C)
            },
            "hello",
        );
        let project = parent.path().join("hello");

        assert!(project.join("firmware.asm").is_file());
        assert!(!project.join("libs").exists());
        assert!(!project.join("software.mk").exists());
    }

    #[test]
    fn a_firmware_project_says_so_where_every_command_will_see_it() {
        let written = project_settings(&ProjectPlan {
            kind: ProjectKind::Firmware,
            ..plan(Board::Rosco6502, ProjectLanguage::Asm)
        });

        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert_eq!(config.project.kind, ProjectKind::Firmware);
    }

    #[test]
    fn the_board_is_recorded_so_every_later_command_knows_it() {
        let parent = create_in(&plan(Board::Rosco6502, ProjectLanguage::C), "hello");
        let settings = fs::read_to_string(parent.path().join("hello").join(CONFIG_FILE)).unwrap();

        let config: crate::config::Config = toml::from_str(&settings).unwrap();
        assert_eq!(config.project.board, Board::Rosco6502);
    }

    #[test]
    fn choices_that_match_the_defaults_are_left_out_of_the_project_file() {
        let written = project_settings(&plan(Board::RoscoM68k, ProjectLanguage::C));

        assert!(written.contains("board = \"rosco_m68k\""), "{written}");
        assert!(!written.contains("toolchain"), "{written}");
        assert!(!written.contains("target"), "{written}");
    }

    #[test]
    fn choices_that_do_not_are_written_down() {
        let written = project_settings(&ProjectPlan {
            toolchain: Toolchain::Host,
            target: Target::Emulator,
            ..plan(Board::Rosco6502, ProjectLanguage::Asm)
        });

        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert_eq!(config.build.toolchain, Toolchain::Host);
        assert_eq!(config.defaults.target, Target::Emulator);
        // Nothing was said about the emulator, so nothing decides it here.
        assert!(!written.contains("[emulator]"), "{written}");
    }

    #[test]
    fn an_emulator_run_from_the_image_needs_no_flag_afterwards() {
        let written = project_settings(&ProjectPlan {
            emulator: EmulatorSetup::Docker,
            ..plan(Board::RoscoM68k, ProjectLanguage::C)
        });

        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert_eq!(config.emulator.source, EmulatorSource::Docker);
        assert!(config.emulator.program.is_none());
    }

    #[test]
    fn an_emulator_on_this_computer_is_recorded_by_path() {
        let written = project_settings(&ProjectPlan {
            emulator: EmulatorSetup::Program("/home/me/rosco-emulator/rosco".into()),
            ..plan(Board::Rosco6502, ProjectLanguage::C)
        });

        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert_eq!(
            config.emulator.program.as_deref(),
            Some(Path::new("/home/me/rosco-emulator/rosco"))
        );
        assert_eq!(config.emulator.source, EmulatorSource::Host);
    }

    #[test]
    fn a_windows_path_survives_being_written_as_toml() {
        let written = project_settings(&ProjectPlan {
            emulator: EmulatorSetup::Program(r"C:\emulator\rosco.exe".into()),
            ..plan(Board::RoscoM68k, ProjectLanguage::C)
        });

        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert_eq!(
            config.emulator.program.as_deref(),
            Some(Path::new(r"C:\emulator\rosco.exe"))
        );
    }

    /// The answers are kept beside the per-user settings, so the environment
    /// that moves those moves these too.
    fn with_settings_home<T>(body: impl FnOnce() -> T) -> T {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: the tests that set this all run under the same lock.
        unsafe { std::env::set_var(config::CONFIG_HOME_ENV, home.path()) };
        body()
    }

    #[test]
    fn the_last_answers_come_back_the_way_they_went_in() {
        let _guard = SETTINGS_HOME.lock().unwrap();
        with_settings_home(|| {
            let plan = ProjectPlan {
                toolchain: Toolchain::Host,
                target: Target::Emulator,
                emulator: EmulatorSetup::Program("/opt/rosco/rosco".into()),
                ..plan(Board::Rosco6502, ProjectLanguage::Asm)
            };

            Remembered::of(&plan).save().unwrap();
            let remembered = Remembered::load().expect("the answers were just saved");

            assert_eq!(remembered.board, Board::Rosco6502);
            assert_eq!(remembered.language, ProjectLanguage::Asm);
            assert_eq!(remembered.toolchain, Toolchain::Host);
            assert_eq!(remembered.target, Target::Emulator);
            assert_eq!(
                remembered.emulator(),
                EmulatorSetup::Program("/opt/rosco/rosco".into())
            );
        });
    }

    #[test]
    fn nothing_remembered_yet_is_not_an_error() {
        let _guard = SETTINGS_HOME.lock().unwrap();
        with_settings_home(|| assert!(Remembered::load().is_none()));
    }

    #[test]
    fn a_memory_that_cannot_be_read_is_the_same_as_none() {
        let _guard = SETTINGS_HOME.lock().unwrap();
        with_settings_home(|| {
            let path = remembered_path().unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "[last]\nboard = \"rosco_z80\"\n").unwrap();

            assert!(Remembered::load().is_none());
        });
    }

    /// `set_var` is process-wide, so the tests that move the settings home
    /// take turns.
    static SETTINGS_HOME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_firmware_summary_leaves_out_what_it_never_chose() {
        let plan = ProjectPlan {
            kind: ProjectKind::Firmware,
            toolchain: Toolchain::Host,
            target: Target::Emulator,
            emulator: EmulatorSetup::Docker,
            ..plan(Board::RoscoM68k, ProjectLanguage::Asm)
        };
        let summary = Remembered::of(&plan).summary();

        assert!(summary.contains("rosco_m68k firmware"), "{summary}");
        // The language and the target were never questions.
        assert!(!summary.contains("asm"), "{summary}");
        assert!(!summary.contains("emulator,"), "{summary}");
    }

    #[test]
    fn the_summary_says_what_was_chosen_on_one_line_of_a_terminal() {
        let plan = ProjectPlan {
            toolchain: Toolchain::Host,
            target: Target::Emulator,
            emulator: EmulatorSetup::Docker,
            ..plan(Board::Rosco6502, ProjectLanguage::Asm)
        };
        let summary = Remembered::of(&plan).summary();

        assert!(summary.contains("rosco_6502"), "{summary}");
        assert!(summary.contains("asm"), "{summary}");
        assert!(summary.contains("host build"), "{summary}");
        assert!(summary.contains("docker emulator"), "{summary}");
        // The question it sits in indents it, and it has to fit beside the
        // label on the narrowest terminal anyone still uses.
        assert!(summary.chars().count() <= 58, "{summary}");
    }

    #[test]
    fn an_existing_directory_is_never_written_into() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("hello");
        fs::create_dir(&destination).unwrap();

        let error = create(&plan(Board::Rosco6502, ProjectLanguage::C), &destination)
            .unwrap_err()
            .to_string();

        assert!(error.contains("already exists"), "{error}");
    }
}
