//! Loading a program into a rosco_6502 board through its firmware monitor.
//!
//! The 6502 firmware has no Kermit receiver. What it has is EWozMon's `L`
//! command, which reads Intel hex records straight into memory and answers
//! each accepted line with a full stop. That answer is the whole flow control
//! there is - the UART has no other - so this sends one record at a time and
//! waits to be told the board kept up.
//!
//! The records are produced here rather than taken from a `.hex` file next to
//! the binary: the address a program loads at is a setting, the binary is what
//! the build promises to produce, and one file is easier to be sure about than
//! two.

use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Bytes per record. Any more and a line grows past what the monitor's input
/// buffer will hold.
const RECORD_LENGTH: usize = 16;

/// What the monitor prints when it is ready to be sent records.
const READY: &str = "Start Intel hex file load:";
/// ... and when it has read the end-of-file record.
const FINISHED: &str = "Load ";
const SUCCEEDED: &str = "successful";

/// Answers to a single record.
const ACCEPTED: u8 = b'.';
const REJECTED: u8 = b'X';

#[derive(Clone, Copy, Debug)]
pub struct HexOptions {
    /// Where the program is loaded, and where it is started afterwards.
    pub load_address: u16,
    /// How long to wait for the monitor to answer.
    pub reply_timeout: Duration,
    /// How many times a record rejected for its checksum is sent again.
    pub max_retries: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferProgress {
    pub name: String,
    pub sent: usize,
    pub total: usize,
}

/// Loads `bytes` into the board and starts them.
///
/// The board has to be sitting at the monitor prompt: that is where it is
/// after a reset, and where a program that returns leaves it.
pub fn send_program<S: Read + Write + ?Sized>(
    console: &mut S,
    name: &str,
    bytes: &[u8],
    options: HexOptions,
    mut progress: impl FnMut(TransferProgress),
) -> Result<()> {
    if bytes.is_empty() {
        bail!("{name} is empty, so there is nothing to load");
    }
    let records = records(bytes, options.load_address)?;

    write_command(console, "L")?;
    wait_for(console, READY, options.reply_timeout).context(
        "the board did not offer to read a hex file. It has to be at the monitor prompt: \
         press reset, or Ctrl-C out of a running program",
    )?;

    progress(TransferProgress {
        name: name.to_string(),
        sent: 0,
        total: bytes.len(),
    });
    for (index, record) in records.iter().enumerate() {
        send_record(console, record, &options)?;
        progress(TransferProgress {
            name: name.to_string(),
            sent: (bytes.len()).min((index + 1) * RECORD_LENGTH),
            total: bytes.len(),
        });
    }

    write_record(console, &end_of_file())?;
    let report = wait_for(console, FINISHED, options.reply_timeout)
        .context("the board did not say how the load went")?;
    let report = read_rest_of_line(console, report, options.reply_timeout);
    if !report.contains(SUCCEEDED) {
        bail!("the board rejected the transfer: {}", report.trim());
    }

    // Nothing else starts it: the monitor has the program in memory and is
    // waiting for the next command.
    write_command(console, &format!("{:04X}R", options.load_address))
}

/// The Intel hex records for `bytes` loaded at `load_address`, without the
/// end-of-file record.
pub fn records(bytes: &[u8], load_address: u16) -> Result<Vec<String>> {
    let end = usize::from(load_address) + bytes.len();
    if end > 0x1_0000 {
        bail!(
            "the program does not fit in memory: {} bytes at ${load_address:04X} would run past $FFFF",
            bytes.len()
        );
    }

    Ok(bytes
        .chunks(RECORD_LENGTH)
        .enumerate()
        .map(|(index, chunk)| {
            let address = load_address + (index * RECORD_LENGTH) as u16;
            record(address, 0x00, chunk)
        })
        .collect())
}

fn end_of_file() -> String {
    record(0, 0x01, &[])
}

fn record(address: u16, kind: u8, data: &[u8]) -> String {
    let mut line = format!(
        ":{:02X}{address:04X}{kind:02X}",
        u8::try_from(data.len()).expect("records are at most 16 bytes")
    );
    for byte in data {
        line.push_str(&format!("{byte:02X}"));
    }

    let sum = (data.len() as u8)
        .wrapping_add((address >> 8) as u8)
        .wrapping_add(address as u8)
        .wrapping_add(kind)
        .wrapping_add(data.iter().copied().fold(0_u8, u8::wrapping_add));
    line.push_str(&format!("{:02X}", sum.wrapping_neg()));
    line
}

/// Sends one record and waits to hear that it arrived intact.
///
/// A rejected record can simply be sent again: every record carries its own
/// address, so the second copy lands exactly where the first one did.
fn send_record<S: Read + Write + ?Sized>(
    console: &mut S,
    record: &str,
    options: &HexOptions,
) -> Result<()> {
    for attempt in 0..=options.max_retries {
        write_record(console, record)?;
        match wait_for_answer(console, options.reply_timeout)? {
            ACCEPTED => return Ok(()),
            REJECTED if attempt < options.max_retries => continue,
            _ => bail!(
                "the board reported a checksum error on the same record {} times; \
                 try a lower serial.baud",
                options.max_retries + 1
            ),
        }
    }
    unreachable!("the loop returns or fails on its last attempt")
}

/// The hex loader ends a record on a line feed and finds the colon itself, so
/// both characters are sent and nothing else has to be true of the line.
fn write_record<S: Read + Write + ?Sized>(console: &mut S, record: &str) -> Result<()> {
    write(console, &format!("{record}\r\n"))
}

/// A command to the monitor ends with a carriage return and nothing else.
///
/// The line feed matters: the loader reads it as the end of a record, and a
/// record with no colon in it is one it counts as damaged. A stray line feed
/// left over from the `L` command would fail the whole transfer.
///
/// The characters go one at a time. The monitor echoes what it is typed and
/// the UART holds only a few bytes, so a command that arrives in one burst
/// while the board is still printing loses characters out of the middle of
/// itself - which is how `0800R` becomes `080`, and a program that was loaded
/// perfectly well never starts.
fn write_command<S: Read + Write + ?Sized>(console: &mut S, command: &str) -> Result<()> {
    settle(console);
    for character in command.bytes().chain(std::iter::once(b'\r')) {
        console
            .write_all(&[character])
            .context("could not write to the board")?;
        console.flush().context("could not flush the board")?;
        std::thread::sleep(KEYSTROKE_PAUSE);
    }
    Ok(())
}

/// Long enough for the board to have echoed the previous character.
const KEYSTROKE_PAUSE: Duration = Duration::from_millis(5);

fn write<S: Read + Write + ?Sized>(console: &mut S, text: &str) -> Result<()> {
    console
        .write_all(text.as_bytes())
        .context("could not write to the board")?;
    console.flush().context("could not flush the board")
}

fn wait_for_answer<S: Read + Write + ?Sized>(console: &mut S, timeout: Duration) -> Result<u8> {
    let deadline = Instant::now() + timeout;
    let mut byte = [0_u8; 1];
    loop {
        match console.read(&mut byte) {
            Ok(1) if byte[0] == ACCEPTED || byte[0] == REJECTED => return Ok(byte[0]),
            // Anything else is the board talking about something else.
            Ok(_) => {}
            Err(error) if is_quiet(&error) => {}
            Err(error) => return Err(error).context("could not read from the board"),
        }
        if Instant::now() >= deadline {
            bail!("the board stopped answering part-way through the transfer");
        }
    }
}

/// Reads until `needle` has been seen, and returns everything read.
fn wait_for<S: Read + Write + ?Sized>(
    console: &mut S,
    needle: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut seen = String::new();
    let mut buffer = [0_u8; 256];
    loop {
        match console.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                seen.push_str(&String::from_utf8_lossy(&buffer[..count]));
                if seen.contains(needle) {
                    return Ok(seen);
                }
            }
            Err(error) if is_quiet(&error) => {}
            Err(error) => return Err(error).context("could not read from the board"),
        }
        if Instant::now() >= deadline {
            bail!("nothing came back within {} seconds", timeout.as_secs());
        }
    }
}

/// Collects what follows a message the board has started printing, so that a
/// failure can be quoted in its own words. Silence just ends the line.
fn read_rest_of_line<S: Read + Write + ?Sized>(
    console: &mut S,
    mut seen: String,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout.min(Duration::from_secs(2));
    let mut buffer = [0_u8; 256];
    while Instant::now() < deadline {
        match console.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => seen.push_str(&String::from_utf8_lossy(&buffer[..count])),
            Err(error) if is_quiet(&error) => {}
            Err(_) => break,
        }
        if seen.contains(SUCCEEDED) || seen.contains("error") {
            break;
        }
    }
    match seen.rfind(FINISHED) {
        Some(start) => seen[start..].to_string(),
        None => seen,
    }
}

/// Waits for the board to stop talking, and throws away what it said.
///
/// A command typed over the top of something the monitor is still printing is
/// a command it may only half hear.
fn settle<S: Read + Write + ?Sized>(console: &mut S) {
    const QUIET: Duration = Duration::from_millis(100);
    let give_up = Instant::now() + Duration::from_secs(2);
    let mut quiet_since = Instant::now();
    let mut buffer = [0_u8; 256];

    while Instant::now() < give_up && Instant::now() - quiet_since < QUIET {
        match console.read(&mut buffer) {
            Ok(count) if count > 0 => quiet_since = Instant::now(),
            _ => {}
        }
    }
}

/// A serial port with nothing to say reports a timeout every time its read
/// window closes, which is normal rather than a fault.
fn is_quiet(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_carries_its_address_and_a_checksum_that_zeroes_the_line() {
        let lines = records(&[0xA9, 0x08], 0x0800).unwrap();
        assert_eq!(lines, [":02080000A90845"]);

        // The whole record, checksum included, sums to zero.
        let line = &lines[0];
        let sum = line.as_bytes()[1..]
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .fold(0_u8, u8::wrapping_add);
        assert_eq!(sum, 0);
    }

    #[test]
    fn long_programs_are_split_into_addressed_records() {
        let lines = records(&[0; 20], 0x0800).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with(":10080000"), "{}", lines[0]);
        assert!(lines[1].starts_with(":04081000"), "{}", lines[1]);
    }

    #[test]
    fn the_end_of_file_record_is_the_one_the_monitor_stops_on() {
        assert_eq!(end_of_file(), ":00000001FF");
    }

    #[test]
    fn a_program_that_would_run_past_the_top_of_memory_is_refused() {
        let error = records(&[0; 0x100], 0xFF80).unwrap_err().to_string();
        assert!(error.contains("does not fit"), "{error}");
    }

    /// Enough of EWozMon to drive the sender through a whole transfer
    /// without a board on the desk, including the two things about it that
    /// are easy to get wrong: a command line ends at the carriage return, and
    /// a line the loader reads without a colon in it damages the transfer.
    struct FakeMonitor {
        input: Vec<u8>,
        output: Vec<u8>,
        loading: bool,
        damaged: bool,
        /// What the records asked to be written, by address.
        memory: Vec<(u16, Vec<u8>)>,
        /// Records to reject once before accepting, by their line.
        reject_once: Vec<String>,
        commands: Vec<String>,
    }

    impl FakeMonitor {
        fn new() -> Self {
            Self {
                input: Vec::new(),
                output: Vec::new(),
                loading: false,
                damaged: false,
                memory: Vec::new(),
                reject_once: Vec::new(),
                commands: Vec::new(),
            }
        }

        fn command(&mut self, line: &str) {
            let line = line.trim();
            if line.is_empty() {
                return;
            }
            self.commands.push(line.to_string());
            if line == "L" {
                self.loading = true;
                self.output
                    .extend_from_slice(b"\r\nStart Intel hex file load:\r\n");
            }
        }

        fn record(&mut self, line: &str) {
            let Some(start) = line.find(':') else {
                // What the firmware does with a line it cannot read: carries
                // on, and remembers that the transfer is no longer sound.
                self.damaged = true;
                return;
            };
            let line = line[start..].trim();

            if let Some(position) = self.reject_once.iter().position(|held| held == line) {
                self.reject_once.remove(position);
                self.output.push(REJECTED);
                return;
            }

            let count = u8::from_str_radix(&line[1..3], 16).unwrap() as usize;
            let address = u16::from_str_radix(&line[3..7], 16).unwrap();
            let kind = u8::from_str_radix(&line[7..9], 16).unwrap();
            if kind == 0x01 {
                self.loading = false;
                self.output.extend_from_slice(if self.damaged {
                    b"\r\nLoad failed with checksum error!\r\n".as_slice()
                } else {
                    b"\r\nLoad successful. Start:0800 Bytes:0016\r\n".as_slice()
                });
                return;
            }

            let data = (0..count)
                .map(|index| u8::from_str_radix(&line[9 + index * 2..11 + index * 2], 16).unwrap())
                .collect();
            self.memory.push((address, data));
            self.output.push(ACCEPTED);
        }
    }

    impl Write for FakeMonitor {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.input.extend_from_slice(buffer);
            loop {
                // The command reader stops at a carriage return; the hex
                // loader reads on until a line feed.
                let terminator = if self.loading { b'\n' } else { b'\r' };
                let Some(end) = self.input.iter().position(|&byte| byte == terminator) else {
                    break;
                };
                let line: Vec<u8> = self.input.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line).into_owned();
                if self.loading {
                    self.record(&line);
                } else {
                    self.command(&line);
                }
            }
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Read for FakeMonitor {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.output.is_empty() {
                return Err(std::io::Error::from(ErrorKind::TimedOut));
            }
            let count = self.output.len().min(buffer.len());
            buffer[..count].copy_from_slice(&self.output[..count]);
            self.output.drain(..count);
            Ok(count)
        }
    }

    fn options() -> HexOptions {
        HexOptions {
            load_address: 0x0800,
            reply_timeout: Duration::from_millis(200),
            max_retries: 2,
        }
    }

    #[test]
    fn a_whole_program_reaches_memory_and_is_then_started() {
        let mut monitor = FakeMonitor::new();
        let program: Vec<u8> = (0..20).collect();
        let mut reported = Vec::new();

        send_program(&mut monitor, "hello.bin", &program, options(), |progress| {
            reported.push(progress.sent)
        })
        .unwrap();

        assert_eq!(
            monitor.memory,
            [
                (0x0800, (0..16).collect::<Vec<u8>>()),
                (0x0810, (16..20).collect::<Vec<u8>>()),
            ]
        );
        assert_eq!(monitor.commands, ["L", "0800R"]);
        assert_eq!(reported, [0, 16, 20]);
        // Nothing the sender wrote was read as a record it could not parse.
        assert!(!monitor.damaged);
    }

    #[test]
    fn a_record_the_board_could_not_read_is_sent_again() {
        let mut monitor = FakeMonitor::new();
        let program = vec![0xEA; 16];
        monitor.reject_once = records(&program, 0x0800).unwrap();

        send_program(&mut monitor, "hello.bin", &program, options(), |_| {}).unwrap();

        assert_eq!(monitor.memory, [(0x0800, program)]);
    }

    #[test]
    fn a_board_that_is_not_at_its_prompt_says_what_to_do_about_it() {
        struct Silent;
        impl Read for Silent {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(ErrorKind::TimedOut))
            }
        }
        impl Write for Silent {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                Ok(buffer.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let error = send_program(
            &mut Silent,
            "hello.bin",
            &[0xEA],
            HexOptions {
                reply_timeout: Duration::from_millis(20),
                ..options()
            },
            |_| {},
        )
        .unwrap_err();

        let message = format!("{error:#}");
        assert!(message.contains("monitor prompt"), "{message}");
    }
}
