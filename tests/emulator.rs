//! End-to-end checks against a real emulator build.
//!
//! These only run when ROSCO_EMULATOR points at the emulator binary, so a
//! checkout without one still passes `cargo test`.

use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rosco_cli::emulator::{self, EmulatorOptions, Runner};
use rosco_cli::ihex::{self, HexOptions};

fn emulator_under_test() -> Option<PathBuf> {
    let program = PathBuf::from(std::env::var_os("ROSCO_EMULATOR")?);
    program.is_file().then_some(program)
}

fn options(program: PathBuf, machine: &str) -> EmulatorOptions {
    EmulatorOptions {
        runner: Runner::Program(program),
        machine: machine.into(),
        program_binary: None,
        firmware: None,
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

/// The route a program takes to a board over the UART: `L` at the monitor
/// prompt, Intel hex records, and the run command. The emulator runs the same
/// firmware a board does, so the conversation is the real one even though the
/// wire is a socket.
#[test]
fn loads_a_program_through_the_firmware_monitor() {
    let Some(program) = emulator_under_test() else {
        eprintln!("ROSCO_EMULATOR not set; skipping");
        return;
    };

    let session = emulator::start(&options(program, "rosco_6502")).expect("the emulator runs");
    let mut console = session.console().expect("the console is attached");
    console
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("the console can time out");

    wait_for_prompt(&mut console);

    ihex::send_program(
        &mut console,
        "ok.bin",
        PROBE,
        HexOptions {
            load_address: 0x0800,
            reply_timeout: Duration::from_secs(10),
            max_retries: 2,
        },
        |_| {},
    )
    .expect("the monitor should accept the program");

    // Printed by the program itself, so seeing it means the records reached
    // memory intact and the run command started them.
    let printed = read_for(&mut console, Duration::from_secs(10), "OK");
    assert!(
        emulator::strip_ansi(&printed).contains("OK"),
        "the loaded program did not run; console was:\n{printed}"
    );
}

/// A program that prints `OK` through the DUART and returns to the monitor:
///
/// ```text
/// _start:  lda #'O'      putchar: pha
///          jsr putchar   wait:    lda $C001   ; status
///          lda #'K'               and #$04    ; transmitter ready
///          jsr putchar            beq wait
///          rts                    pla
///                                 sta $C003   ; transmit
///                                 rts
/// ```
const PROBE: &[u8] = &[
    0xa9, 0x4f, 0x20, 0x0b, 0x08, 0xa9, 0x4b, 0x20, 0x0b, 0x08, 0x60, 0x48, 0xad, 0x01, 0xc0, 0x29,
    0x04, 0xf0, 0xf9, 0x68, 0x8d, 0x03, 0xc0, 0x60,
];

/// Reads until the monitor says it is listening, so that the first character
/// of the transfer is not sent into a board that is still booting.
fn wait_for_prompt(console: &mut impl Read) {
    let seen = read_for(console, Duration::from_secs(30), "EWozMon");
    assert!(
        seen.contains("EWozMon"),
        "the firmware never reached its monitor; console was:\n{seen}"
    );
}

/// Everything the machine says until it says `needle`, or until the time is up.
fn read_for(console: &mut impl Read, timeout: Duration, needle: &str) -> String {
    let deadline = Instant::now() + timeout;
    let mut seen = String::new();
    let mut buffer = [0_u8; 512];
    while Instant::now() < deadline {
        if let Ok(count) = console.read(&mut buffer) {
            seen.push_str(&String::from_utf8_lossy(&buffer[..count]));
            if seen.contains(needle) {
                break;
            }
        }
    }
    seen
}
