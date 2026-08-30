//! The compilers a board needs when the build runs on this computer.
//!
//! Docker is the reproducible path and stays the default, but the 6502
//! toolchain is one package away on every desktop system, and a build that
//! does not have to start a container is worth having. This module knows what
//! has to be present and, when something is not, what to type to get it.

use std::process::{Command, Stdio};

use crate::config::Board;

/// A program the build calls by name.
#[derive(Clone, Copy, Debug)]
pub struct Tool {
    pub command: &'static str,
    /// What it does, for the line `doctor` prints about it.
    pub purpose: &'static str,
}

const M68K_TOOLS: &[Tool] = &[
    Tool {
        command: "make",
        purpose: "build driver",
    },
    Tool {
        command: "m68k-elf-rosco-gcc",
        purpose: "C compiler",
    },
    Tool {
        command: "m68k-elf-rosco-ld",
        purpose: "linker",
    },
    Tool {
        command: "vasmm68k_mot",
        purpose: "assembler",
    },
];

const MOS6502_TOOLS: &[Tool] = &[
    Tool {
        command: "make",
        purpose: "build driver",
    },
    Tool {
        command: "ca65",
        purpose: "assembler",
    },
    Tool {
        command: "ld65",
        purpose: "linker",
    },
    Tool {
        command: "cl65",
        purpose: "C compiler and linker",
    },
];

pub fn required(board: Board) -> &'static [Tool] {
    match board {
        Board::RoscoM68k => M68K_TOOLS,
        Board::Rosco6502 => MOS6502_TOOLS,
    }
}

pub fn missing(board: Board) -> Vec<&'static Tool> {
    required(board)
        .iter()
        .filter(|tool| !available(tool.command))
        .collect()
}

/// Whether a program of this name can be started at all.
///
/// Only that it runs is interesting; tools differ on what they make of
/// `--version` and some of them report failure after printing it.
pub fn available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// The command that installs a board's toolchain on this system, when there
/// is one worth recommending.
pub fn install_hint(board: Board) -> Option<String> {
    match board {
        // The rosco_m68k cross-compiler is not in any distribution's archive;
        // Homebrew's tap is the only supported way to install it.
        Board::RoscoM68k => (!cfg!(windows)).then(|| {
            "brew tap rosco-m68k/toolchain && \
             brew install rosco-m68k-toolchain@13 vasm-all srecord"
                .to_string()
        }),
        Board::Rosco6502 => package_manager().map(|manager| manager.install("cc65")),
    }
}

/// What this system installs packages with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Homebrew,
}

impl PackageManager {
    pub fn install(self, package: &str) -> String {
        match self {
            Self::Apt => format!("sudo apt install {package}"),
            Self::Dnf => format!("sudo dnf install {package}"),
            Self::Pacman => format!("sudo pacman -S {package}"),
            Self::Zypper => format!("sudo zypper install {package}"),
            Self::Homebrew => format!("brew install {package}"),
        }
    }
}

fn package_manager() -> Option<PackageManager> {
    if cfg!(target_os = "macos") {
        return Some(PackageManager::Homebrew);
    }
    if cfg!(windows) {
        return None;
    }

    let release = std::fs::read_to_string("/etc/os-release").ok()?;
    from_os_release(&release)
}

/// Reads the distribution family out of `/etc/os-release`. `ID_LIKE` is what
/// makes this work on the derivatives, which outnumber their parents.
fn from_os_release(release: &str) -> Option<PackageManager> {
    let names: Vec<String> = release
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| *key == "ID" || *key == "ID_LIKE")
        .flat_map(|(_, value)| {
            value
                .trim_matches('"')
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect();

    names.iter().find_map(|name| match name.as_str() {
        "debian" | "ubuntu" => Some(PackageManager::Apt),
        "fedora" | "rhel" | "centos" => Some(PackageManager::Dnf),
        "arch" => Some(PackageManager::Pacman),
        "opensuse" | "suse" => Some(PackageManager::Zypper),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_derivative_is_recognised_by_the_family_it_names() {
        let mint = "NAME=\"Linux Mint\"\nID=linuxmint\nID_LIKE=\"ubuntu debian\"\n";
        assert_eq!(from_os_release(mint), Some(PackageManager::Apt));
    }

    #[test]
    fn its_own_id_is_enough_when_there_is_no_family() {
        assert_eq!(
            from_os_release("ID=arch\nNAME=\"Arch Linux\"\n"),
            Some(PackageManager::Pacman)
        );
    }

    #[test]
    fn an_unknown_distribution_gets_no_advice_rather_than_wrong_advice() {
        assert_eq!(from_os_release("ID=plan9\n"), None);
    }

    #[test]
    fn the_6502_toolchain_is_one_package() {
        assert_eq!(PackageManager::Apt.install("cc65"), "sudo apt install cc65");
    }

    #[test]
    fn each_board_names_the_programs_its_build_calls() {
        assert!(
            required(Board::Rosco6502)
                .iter()
                .any(|tool| tool.command == "ca65")
        );
        assert!(
            required(Board::RoscoM68k)
                .iter()
                .any(|tool| tool.command == "m68k-elf-rosco-gcc")
        );
    }
}
