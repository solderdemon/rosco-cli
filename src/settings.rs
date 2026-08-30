//! `rosco config`: settings you save once instead of passing them every run.
//!
//! The per-user file holds what belongs to this workstation and applies to
//! every project; `rosco.toml` holds what belongs to the application and wins
//! where they overlap. Command-line flags still beat both.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item, Table};

use crate::cli::{ConfigCommand, ScopeArgs};
use crate::config::{self, CONFIG_FILE, Config, Layers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project,
}

impl Scope {
    fn from_args(args: &ScopeArgs) -> Self {
        if args.local {
            Self::Project
        } else {
            Self::Global
        }
    }
}

pub fn run(project_root: Option<&Path>, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::List => list(project_root),
        ConfigCommand::Get { key } => get(project_root, &key),
        ConfigCommand::Set { key, value, scope } => {
            let path = scope_path(Scope::from_args(&scope), project_root)?;
            let stored = set(&path, &key, &value)?;
            eprintln!("{key} = {stored} saved to {}", path.display());
            Ok(())
        }
        ConfigCommand::Unset { key, scope } => {
            let path = scope_path(Scope::from_args(&scope), project_root)?;
            unset(&path, &key)?;
            eprintln!("Removed {key} from {}", path.display());
            Ok(())
        }
        ConfigCommand::Path { scope } => {
            let path = scope_path(Scope::from_args(&scope), project_root)?;
            println!("{}", path.display());
            Ok(())
        }
        ConfigCommand::Edit { scope } => edit(Scope::from_args(&scope), project_root),
    }
}

fn scope_path(scope: Scope, project_root: Option<&Path>) -> Result<PathBuf> {
    match scope {
        Scope::Global => config::global_config_path(),
        Scope::Project => Ok(project_root
            .context("no project directory; run this inside one or pass -C")?
            .join(CONFIG_FILE)),
    }
}

fn layers(project_root: Option<&Path>) -> Result<Layers> {
    Layers::load_from(config::global_config_path().ok().as_deref(), project_root)
}

fn list(project_root: Option<&Path>) -> Result<()> {
    let layers = layers(project_root)?;
    for (label, path, present) in [
        (
            "global ",
            layers.global_path.as_deref(),
            layers.global.is_some(),
        ),
        (
            "project",
            layers.project_path.as_deref(),
            layers.project.is_some(),
        ),
    ] {
        match path {
            Some(path) if present => println!("# {label}  {}", path.display()),
            Some(path) => println!("# {label}  {} (not created yet)", path.display()),
            None => println!("# {label}  <none>"),
        }
    }
    println!();

    let config = layers.config()?;
    let mut settings: Vec<_> = config::flatten(&config.to_value()?)
        .into_iter()
        .map(|(key, value)| (key, render_toml(&value)))
        .collect();
    for key in config::OPTIONAL_SETTINGS {
        if !settings.iter().any(|(known, _)| known == key) {
            settings.push((key.to_string(), "(unset)".to_string()));
        }
    }
    settings.sort_by(|(left, _), (right, _)| left.cmp(right));
    let keys = settings.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    let values = settings
        .iter()
        .map(|(_, value)| value.len())
        .max()
        .unwrap_or(0);
    for (key, value) in &settings {
        println!("{key:keys$}  {value:values$}  {}", layers.origin(key));
    }
    if config.build.environment.is_empty() {
        println!("\nbuild.environment.<NAME> passes a variable to the build process.");
    }
    Ok(())
}

/// Prints strings unquoted so the value can be used in a script.
fn get(project_root: Option<&Path>, key: &str) -> Result<()> {
    let layers = layers(project_root)?;
    let value = layers.config()?.to_value()?;
    let Some(found) = config::lookup(value.as_table().expect("settings are a table"), key) else {
        bail!("{key} is not set; `rosco config list` shows the settings that are");
    };
    println!("{}", render_plain(found));
    Ok(())
}

/// Writes `key = value`, leaving the rest of the file, comments included, alone.
fn set(path: &Path, key: &str, raw: &str) -> Result<String> {
    check_key(key)?;
    let mut document = read_document(path)?;

    let mut rejection = None;
    for candidate in candidates(raw) {
        let mut trial = document.clone();
        insert(&mut trial, key, candidate.clone())?;
        match validate(&trial) {
            Ok(()) => {
                document = trial;
                write_document(path, &document)?;
                return Ok(candidate.to_string().trim().to_string());
            }
            Err(error) => rejection = rejection.or(Some(error)),
        }
    }

    Err(rejection.expect("at least one candidate is always tried"))
}

fn unset(path: &Path, key: &str) -> Result<()> {
    check_key(key)?;
    if !path.exists() {
        bail!(
            "{} does not exist, so {key} is not set there",
            path.display()
        );
    }

    let mut document = read_document(path)?;
    let (parents, leaf) = split_key(key);
    let mut table = document.as_table_mut();
    for parent in parents {
        table = match table.get_mut(parent).and_then(Item::as_table_mut) {
            Some(table) => table,
            None => bail!("{key} is not set in {}", path.display()),
        };
    }
    if table.remove(leaf).is_none() {
        bail!("{key} is not set in {}", path.display());
    }

    prune(document.as_table_mut());
    validate(&document)?;
    write_document(path, &document)?;
    Ok(())
}

fn edit(scope: Scope, project_root: Option<&Path>) -> Result<()> {
    let path = scope_path(scope, project_root)?;
    if !path.exists() {
        let starter = match scope {
            Scope::Global => "# rosco settings for this user, shared by every project.\n",
            Scope::Project => "# rosco settings for this project.\n",
        };
        write_new(&path, starter)?;
    }

    let editor = std::env::var_os("VISUAL")
        .or_else(|| std::env::var_os("EDITOR"))
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "vi".into()
            }
        });
    let status = Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("could not start {}", editor.to_string_lossy()))?;
    if !status.success() {
        bail!("{} exited with {status}", editor.to_string_lossy());
    }

    // Finding out now beats finding out in the middle of a build.
    let source =
        fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
    toml::from_str::<Config>(&source).with_context(|| format!("invalid {}", path.display()))?;
    Ok(())
}

fn check_key(key: &str) -> Result<()> {
    if key.is_empty() || key.split('.').any(str::is_empty) {
        bail!("{key:?} is not a setting name; use a dotted name such as serial.port");
    }
    Ok(())
}

fn split_key(key: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let leaf = parts.pop().expect("split always yields one part");
    (parts, leaf)
}

/// `38400` is an integer and `COM3` is a string, but only the schema knows
/// which one a given setting wants, so offer both and let it choose.
fn candidates(raw: &str) -> Vec<toml_edit::Value> {
    let mut candidates = Vec::new();
    if let Ok(parsed) = raw.trim().parse::<toml_edit::Value>() {
        candidates.push(parsed.decorated(" ", ""));
    }
    let literal = toml_edit::Value::from(raw).decorated(" ", "");
    if candidates.first().map(ToString::to_string) != Some(literal.to_string()) {
        candidates.push(literal);
    }
    candidates
}

fn insert(document: &mut DocumentMut, key: &str, value: toml_edit::Value) -> Result<()> {
    let (parents, leaf) = split_key(key);
    let mut table = document.as_table_mut();
    for (depth, parent) in parents.iter().enumerate() {
        let entry = table.entry(parent).or_insert_with(|| {
            let mut created = Table::new();
            // A section that only holds other sections needs no header of its
            // own; one that holds the value we are writing does.
            created.set_implicit(depth + 1 < parents.len());
            Item::Table(created)
        });
        table = entry
            .as_table_mut()
            .with_context(|| format!("{parent} is not a section, so {key} cannot be set"))?;
    }
    table.insert(leaf, toml_edit::value(value));
    Ok(())
}

/// Drops sections left empty by `unset` so files do not collect husks.
fn prune(table: &mut Table) {
    let empty: Vec<String> = table
        .iter_mut()
        .filter_map(|(key, item)| {
            let child = item.as_table_mut()?;
            prune(child);
            child.is_empty().then(|| key.to_string())
        })
        .collect();
    for key in empty {
        table.remove(&key);
    }
}

fn validate(document: &DocumentMut) -> Result<()> {
    let rendered = document.to_string();
    match toml::from_str::<Config>(&rendered) {
        Ok(_) => Ok(()),
        Err(error) => {
            let message = error.to_string();
            if message.contains("unknown field") {
                bail!("{message}\n`rosco config list` shows every setting there is.");
            }
            bail!(message)
        }
    }
}

fn read_document(path: &Path) -> Result<DocumentMut> {
    let source = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?
    } else {
        String::new()
    };
    source
        .parse()
        .with_context(|| format!("invalid {}", path.display()))
}

fn write_document(path: &Path, document: &DocumentMut) -> Result<()> {
    write_new(path, &document.to_string())
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("could not write {}", path.display()))
}

fn render_toml(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => format!("{text:?}"),
        toml::Value::Integer(number) => number.to_string(),
        toml::Value::Float(number) => number.to_string(),
        toml::Value::Boolean(flag) => flag.to_string(),
        toml::Value::Datetime(stamp) => stamp.to_string(),
        toml::Value::Array(items) => {
            let rendered: Vec<_> = items.iter().map(render_toml).collect();
            format!("[{}]", rendered.join(", "))
        }
        toml::Value::Table(_) => "{ ... }".to_string(),
    }
}

fn render_plain(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        other => render_toml(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        if !contents.is_empty() {
            fs::write(&path, contents).unwrap();
        }
        (dir, path)
    }

    #[test]
    fn writing_to_a_missing_file_creates_it_with_the_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");

        set(&path, "serial.port", "/dev/ttyUSB0").unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[serial]\nport = \"/dev/ttyUSB0\"\n"
        );
    }

    #[test]
    fn values_take_the_type_the_setting_expects() {
        let (_dir, path) = file("");
        set(&path, "serial.baud", "115200").unwrap();
        set(&path, "defaults.target", "emulator").unwrap();
        set(&path, "build.args", r#"["all", "-j4"]"#).unwrap();

        let config: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config.serial.baud, 115_200);
        assert_eq!(config.defaults.target, config::Target::Emulator);
        assert_eq!(config.build.args, ["all", "-j4"]);
    }

    #[test]
    fn a_numeric_looking_port_stays_a_string() {
        let (_dir, path) = file("");
        set(&path, "serial.port", "3").unwrap();

        let config: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config.serial.port.as_deref(), Some("3"));
    }

    #[test]
    fn editing_keeps_the_comments_and_the_rest_of_the_file() {
        let (_dir, path) = file(
            "# hand written\n[serial]\n# the adapter on my desk\nport = \"/dev/ttyUSB0\"\nbaud = 38400\n",
        );

        set(&path, "serial.baud", "115200").unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# hand written\n[serial]\n# the adapter on my desk\nport = \"/dev/ttyUSB0\"\nbaud = 115200\n"
        );
    }

    #[test]
    fn nested_sections_get_their_own_header_only_where_it_is_needed() {
        let (_dir, path) = file("");
        set(&path, "build.docker.image", "example/toolchain:v1").unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[build.docker]\nimage = \"example/toolchain:v1\"\n"
        );
    }

    #[test]
    fn a_misspelled_setting_is_refused_and_nothing_is_written() {
        let (_dir, path) = file("");
        let error = set(&path, "serial.prot", "COM3").unwrap_err().to_string();

        assert!(error.contains("unknown field"), "{error}");
        assert!(error.contains("rosco config list"), "{error}");
        assert!(!path.exists());
    }

    #[test]
    fn a_value_of_the_wrong_type_is_refused() {
        let (_dir, path) = file("");
        let error = set(&path, "serial.baud", "fast").unwrap_err().to_string();

        assert!(
            error.contains("baud") || error.contains("integer"),
            "{error}"
        );
    }

    #[test]
    fn unsetting_removes_the_key_and_the_section_it_emptied() {
        let (_dir, path) =
            file("[serial]\nport = \"COM3\"\n\n[emulator]\nmachine = \"rosco_6502\"\n");

        unset(&path, "serial.port").unwrap();

        let left = fs::read_to_string(&path).unwrap();
        assert!(!left.contains("serial"), "{left}");
        assert!(left.contains("rosco_6502"), "{left}");
    }

    #[test]
    fn unsetting_something_that_is_not_there_says_so() {
        let (_dir, path) = file("[serial]\nport = \"COM3\"\n");
        let error = unset(&path, "serial.baud").unwrap_err().to_string();
        assert!(error.contains("not set"), "{error}");
    }

    #[test]
    fn dotted_names_are_required() {
        let (_dir, path) = file("");
        assert!(set(&path, "serial.", "COM3").is_err());
        assert!(set(&path, "", "COM3").is_err());
    }
}
