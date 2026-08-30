//! End-to-end checks against a real emulator build.
//!
//! These only run when ROSCO_EMULATOR points at the emulator binary, so a
//! checkout without one still passes `cargo test`.

use std::path::PathBuf;

use rosco_cli::emulator::{self, EmulatorOptions};

fn emulator_under_test() -> Option<PathBuf> {
    let program = PathBuf::from(std::env::var_os("ROSCO_EMULATOR")?);
    program.is_file().then_some(program)
}

fn options(program: PathBuf, machine: &str) -> EmulatorOptions {
    EmulatorOptions {
        program,
        machine: machine.into(),
        program_binary: None,
        rom_path: None,
        sd_card: None,
        extra_args: Vec::new(),
    }
}

#[test]
fn captures_the_firmware_power_on_self_test() {
    let Some(program) = emulator_under_test() else {
        eprintln!("ROSCO_EMULATOR not set; skipping");
        return;
    };

    let output =
        emulator::capture(&options(program, "rosco_6502"), 8).expect("the emulator should run");

    let plain = emulator::strip_ansi(&output);
    assert!(
        plain.contains("Memory checks: passed"),
        "firmware self test did not pass; console was:\n{output}"
    );
}

#[test]
fn runs_a_program_loaded_straight_into_memory() {
    let Some(program) = emulator_under_test() else {
        eprintln!("ROSCO_EMULATOR not set; skipping");
        return;
    };
    let Some(binary) = std::env::var_os("ROSCO_TEST_PROGRAM").map(PathBuf::from) else {
        eprintln!("ROSCO_TEST_PROGRAM not set; skipping");
        return;
    };

    let mut options = options(program, "rosco_6502");
    options.program_binary = Some(binary);

    let output = emulator::capture(&options, 8).expect("the emulator should run");

    assert!(
        emulator::strip_ansi(&output).contains("QUICKLOAD OK"),
        "the loaded program did not run; console was:\n{output}"
    );
}

#[test]
fn reports_a_machine_that_does_not_exist() {
    let Some(program) = emulator_under_test() else {
        eprintln!("ROSCO_EMULATOR not set; skipping");
        return;
    };

    let error = emulator::capture(&options(program, "no_such_machine"), 2)
        .expect_err("an unknown machine should fail");

    assert!(
        error.to_string().contains("exited before connecting"),
        "unexpected error: {error:#}"
    );
}
