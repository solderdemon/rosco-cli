use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use include_dir::{Dir, DirEntry, include_dir};

use crate::cli::ProjectLanguage;

static COMMON_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/common");
static C_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/c");
static ASM_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/asm");

pub fn create(language: ProjectLanguage, destination: &Path) -> Result<()> {
    if fs::symlink_metadata(destination).is_ok() {
        bail!("destination already exists: {}", destination.display());
    }

    copy_dir(&COMMON_TEMPLATE, destination)?;
    copy_dir(
        match language {
            ProjectLanguage::C => &C_TEMPLATE,
            ProjectLanguage::Asm => &ASM_TEMPLATE,
        },
        destination,
    )?;
    Ok(())
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

    #[test]
    fn includes_common_and_language_files() {
        let destination =
            std::env::temp_dir().join(format!("rosco-init-test-{}", std::process::id()));
        if destination.exists() {
            fs::remove_dir_all(&destination).unwrap();
        }
        create(ProjectLanguage::C, &destination).unwrap();
        assert!(destination.join("software.mk").is_file());
        assert!(destination.join("libs/Makefile").is_file());
        assert!(destination.join("kmain.c").is_file());
        assert!(destination.join("Makefile").is_file());
        fs::remove_dir_all(destination).unwrap();
    }
}
