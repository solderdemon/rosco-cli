use std::io::{ErrorKind, IsTerminal, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serialport::{DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits};

#[derive(Clone, Debug)]
pub struct PortSummary {
    pub name: String,
    pub kind: String,
    pub is_usb: bool,
}

pub fn list_ports() -> Result<Vec<PortSummary>> {
    let ports = serialport::available_ports().context("could not enumerate serial ports")?;
    Ok(ports
        .into_iter()
        .map(|port| {
            let (kind, is_usb) = match port.port_type {
                SerialPortType::UsbPort(usb) => {
                    let mut details = vec![format!("USB {:04x}:{:04x}", usb.vid, usb.pid)];
                    details.extend(usb.manufacturer.filter(|value| !value.is_empty()));
                    details.extend(usb.product.filter(|value| !value.is_empty()));
                    if let Some(serial) = usb.serial_number.filter(|value| !value.is_empty()) {
                        details.push(format!("serial {serial}"));
                    }
                    (details.join(" · "), true)
                }
                SerialPortType::BluetoothPort => ("Bluetooth".into(), false),
                SerialPortType::PciPort => ("PCI".into(), false),
                SerialPortType::Unknown => ("Serial".into(), false),
            };
            PortSummary {
                name: port.port_name,
                kind,
                is_usb,
            }
        })
        .collect())
}

pub fn open(name: &str, baud: u32, timeout: Duration) -> Result<Box<dyn SerialPort>> {
    serialport::new(name, baud)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        .timeout(timeout)
        .open()
        .with_context(|| format!("could not open serial port {name} at {baud} baud"))
}

struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    fn enter() -> Result<Self> {
        if std::io::stdin().is_terminal() {
            crossterm::terminal::enable_raw_mode().context("could not enable terminal raw mode")?;
            Ok(Self { active: true })
        } else {
            Ok(Self { active: false })
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

/// What a console session is talking to, which decides how a quiet read and a
/// closed stream should be treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleKind {
    /// A serial port: reads time out constantly and that is normal.
    Uart,
    /// The emulator over a socket: a closed stream means it exited.
    Emulator,
}

impl ConsoleKind {
    fn banner(self) -> &'static str {
        match self {
            Self::Uart => "Interactive UART session; press Ctrl-] to exit.",
            Self::Emulator => "Emulator console; press Ctrl-] to exit.",
        }
    }

    fn closing(self) -> &'static str {
        match self {
            Self::Uart => "\r\nUART session closed.",
            Self::Emulator => "\r\nEmulator stopped.",
        }
    }
}

pub fn monitor(port: Box<dyn SerialPort>, watch: &Watch) -> Result<()> {
    let writer = port
        .try_clone()
        .context("could not clone serial port for interactive input")?;
    console(port, Box::new(writer), ConsoleKind::Uart, watch)
}

/// What to watch the console for, so a session can end by itself and say
/// whether the program did what it should.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    /// Text whose appearance means the program worked.
    pub expect: Option<String>,
    /// Text whose appearance means it did not.
    pub fail_on: Option<String>,
    /// How long the session may last.
    pub timeout: Option<Duration>,
}

/// How long `--expect` waits when no `--timeout` says otherwise: a CI job
/// that hangs forever on a program that never answers is worse than one
/// that fails.
pub const DEFAULT_EXPECT_TIMEOUT: Duration = Duration::from_secs(60);

impl Watch {
    pub fn new(expect: Option<String>, fail_on: Option<String>, seconds: Option<u64>) -> Self {
        let timeout = match (seconds, &expect) {
            (Some(seconds), _) => Some(Duration::from_secs(seconds)),
            (None, Some(_)) => Some(DEFAULT_EXPECT_TIMEOUT),
            (None, None) => None,
        };
        Self {
            expect,
            fail_on,
            timeout,
        }
    }

    fn is_set(&self) -> bool {
        self.expect.is_some() || self.fail_on.is_some() || self.timeout.is_some()
    }

    fn describe(&self) -> Option<String> {
        let timeout = self.timeout.map(|limit| format!("{} s", limit.as_secs()));
        match (&self.expect, timeout) {
            (Some(text), Some(limit)) => Some(format!("Waiting up to {limit} for {text:?}.")),
            (Some(text), None) => Some(format!("Waiting for {text:?}.")),
            (None, Some(limit)) => Some(format!("Stopping after {limit}.")),
            (None, None) => None,
        }
    }
}

/// How a watched session ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Expected,
    FailedOn,
}

/// Looks for the watched text in what the machine prints, across however
/// many reads it arrives in and with the firmware's colour escapes removed.
struct Watcher {
    watch: Watch,
    recent: Vec<u8>,
    started: Instant,
}

/// How much of the console is kept to match against. A pattern split across
/// reads is still found, while a long session does not grow without limit.
const WATCH_WINDOW: usize = 16 * 1024;

impl Watcher {
    fn new(watch: Watch) -> Self {
        Self {
            watch,
            recent: Vec::new(),
            started: Instant::now(),
        }
    }

    fn feed(&mut self, output: &[u8]) -> Option<Verdict> {
        if self.watch.expect.is_none() && self.watch.fail_on.is_none() {
            return None;
        }
        self.recent.extend_from_slice(output);
        if self.recent.len() > WATCH_WINDOW {
            let excess = self.recent.len() - WATCH_WINDOW;
            self.recent.drain(..excess);
        }

        let text = crate::emulator::strip_ansi(&String::from_utf8_lossy(&self.recent));
        // A failure printed in the same read as the success still counts.
        if let Some(fail) = &self.watch.fail_on
            && text.contains(fail.as_str())
        {
            return Some(Verdict::FailedOn);
        }
        if let Some(expect) = &self.watch.expect
            && text.contains(expect.as_str())
        {
            return Some(Verdict::Expected);
        }
        None
    }

    fn timed_out(&self) -> bool {
        self.watch
            .timeout
            .is_some_and(|limit| self.started.elapsed() >= limit)
    }

    /// The result of a session that ended without the watched text: by
    /// timing out, the machine going away, or Ctrl-].
    fn ended(&self, how: &str) -> Result<()> {
        match &self.watch.expect {
            Some(text) => bail!("{how} before the machine printed {text:?}"),
            None => Ok(()),
        }
    }

    fn concluded(&self, verdict: Verdict) -> Result<()> {
        match verdict {
            Verdict::Expected => Ok(()),
            Verdict::FailedOn => bail!(
                "the machine printed {:?}",
                self.watch.fail_on.as_deref().unwrap_or_default()
            ),
        }
    }
}

/// Drives an interactive session over anything that reads and writes bytes.
pub fn console(
    mut reader: Box<dyn Read + Send>,
    mut writer: Box<dyn Write + Send>,
    kind: ConsoleKind,
    watch: &Watch,
) -> Result<()> {
    eprintln!("{}", kind.banner());
    if let Some(description) = watch.describe() {
        eprintln!("{description}");
    }
    let mut watcher = Watcher::new(watch.clone());
    let mut outcome = None;

    let raw_guard = RawModeGuard::enter()?;
    let is_raw = raw_guard.active;

    let running = Arc::new(AtomicBool::new(true));
    let running_writer = running.clone();

    let stdin_thread = std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0_u8; 512];

        while running_writer.load(Ordering::Relaxed) {
            match stdin.read(&mut buffer) {
                // Stdin being closed or redirected is a fine way to watch a
                // machine boot, so its end only stops forwarding input.
                Ok(0) => return,
                Ok(count) => {
                    let (forward, quit) = split_at_quit(&buffer[..count]);
                    if !forward.is_empty() {
                        if writer.write_all(forward).is_err() {
                            return;
                        }
                        let _ = writer.flush();
                    }
                    if quit {
                        running_writer.store(false, Ordering::SeqCst);
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let mut stdout = std::io::stdout().lock();
    let mut buffer = [0_u8; 4096];
    let mut last_was_cr = false;

    while running.load(Ordering::SeqCst) {
        if watcher.timed_out() {
            outcome = Some(End::TimedOut);
            break;
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                // A serial port simply had nothing to say; a socket is closed.
                if kind == ConsoleKind::Emulator {
                    outcome = Some(End::Closed);
                    break;
                }
            }
            Ok(count) => {
                if is_raw {
                    let shown = raw_newlines(&buffer[..count], &mut last_was_cr);
                    let _ = stdout.write_all(&shown);
                } else {
                    let _ = stdout.write_all(&buffer[..count]);
                }
                let _ = stdout.flush();

                if let Some(verdict) = watcher.feed(&buffer[..count]) {
                    outcome = Some(End::Watched(verdict));
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) => {}
            Err(error) => {
                running.store(false, Ordering::SeqCst);
                drop(raw_guard);
                return Err(error).context("console read failed");
            }
        }
    }

    running.store(false, Ordering::SeqCst);
    drop(raw_guard);
    eprintln!("{}", kind.closing());
    let _ = stdin_thread;

    if !watch.is_set() {
        return Ok(());
    }
    match outcome {
        Some(End::Watched(verdict)) => watcher.concluded(verdict),
        Some(End::TimedOut) => watcher.ended(&format!(
            "{} s passed",
            watch.timeout.unwrap_or_default().as_secs()
        )),
        Some(End::Closed) => watcher.ended("the emulator stopped"),
        None => watcher.ended("the session was closed"),
    }
}

/// Why the console loop stopped, when it was not the user leaving.
enum End {
    Watched(Verdict),
    TimedOut,
    Closed,
}

/// Ctrl-], the byte that ends a session. Ctrl-C is not it: in raw mode it is
/// an ordinary byte, and the program on the board may be waiting for it.
const QUIT: u8 = 0x1d;

/// What of a chunk of keyboard input goes to the machine, and whether the
/// session ends after it. Anything typed before Ctrl-] is still sent.
fn split_at_quit(input: &[u8]) -> (&[u8], bool) {
    match input.iter().position(|&byte| byte == QUIT) {
        Some(end) => (&input[..end], true),
        None => (input, false),
    }
}

/// Turns the bare line feeds a machine prints into CR LF, which a terminal in
/// raw mode needs to start the next line at its left edge. A CR LF the
/// machine sent itself is left as it is, even when a read splits the pair.
fn raw_newlines(output: &[u8], last_was_cr: &mut bool) -> Vec<u8> {
    let mut shown = Vec::with_capacity(output.len());
    for &byte in output {
        if byte == b'\n' && !*last_was_cr {
            shown.extend_from_slice(b"\r\n");
        } else {
            shown.push(byte);
        }
        *last_was_cr = byte == b'\r';
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_is_passed_to_the_machine() {
        assert_eq!(split_at_quit(b"abc\x03"), (&b"abc\x03"[..], false));
    }

    #[test]
    fn ctrl_close_bracket_ends_the_session_after_what_came_before_it() {
        assert_eq!(split_at_quit(b"ls\r\x1dmore"), (&b"ls\r"[..], true));
        assert_eq!(split_at_quit(b"\x1d"), (&b""[..], true));
    }

    fn watcher(expect: Option<&str>, fail_on: Option<&str>) -> Watcher {
        Watcher::new(Watch::new(
            expect.map(str::to_string),
            fail_on.map(str::to_string),
            None,
        ))
    }

    #[test]
    fn expected_text_is_found_across_reads_and_colour_escapes() {
        let mut watcher = watcher(Some("All tests passed"), None);
        assert_eq!(watcher.feed(b"All te"), None);
        assert_eq!(watcher.feed(b"sts \x1b[1;32mpas"), None);
        assert_eq!(watcher.feed(b"sed\r\n"), Some(Verdict::Expected));
    }

    #[test]
    fn a_failure_wins_over_a_success_in_the_same_read() {
        let mut watcher = watcher(Some("PASS"), Some("FAIL"));
        assert_eq!(
            watcher.feed(b"FAIL 1\r\nPASS 2\r\n"),
            Some(Verdict::FailedOn)
        );
        assert!(watcher.concluded(Verdict::FailedOn).is_err());
    }

    #[test]
    fn ending_without_the_expected_text_is_a_failure() {
        let watcher = watcher(Some("PASS"), None);
        let error = watcher.ended("5 s passed").unwrap_err().to_string();
        assert!(error.contains("PASS"), "{error}");
    }

    #[test]
    fn a_plain_timeout_is_not_a_failure() {
        let watch = Watch::new(None, None, Some(5));
        assert_eq!(watch.timeout, Some(Duration::from_secs(5)));
        assert!(Watcher::new(watch).ended("5 s passed").is_ok());
    }

    #[test]
    fn expect_without_a_timeout_still_gives_up_eventually() {
        let watch = Watch::new(Some("PASS".into()), None, None);
        assert_eq!(watch.timeout, Some(DEFAULT_EXPECT_TIMEOUT));
        assert_eq!(Watch::new(None, None, None).timeout, None);
    }

    #[test]
    fn the_window_keeps_the_end_of_a_long_session() {
        let mut watcher = watcher(Some("DONE"), None);
        assert_eq!(watcher.feed(&vec![b'.'; WATCH_WINDOW * 2]), None);
        assert!(watcher.recent.len() <= WATCH_WINDOW);
        assert_eq!(watcher.feed(b"DONE"), Some(Verdict::Expected));
    }

    #[test]
    fn bare_line_feeds_become_cr_lf() {
        let mut last_was_cr = false;
        assert_eq!(raw_newlines(b"a\nb\n", &mut last_was_cr), b"a\r\nb\r\n");
    }

    #[test]
    fn a_cr_lf_split_across_reads_is_not_doubled() {
        let mut last_was_cr = false;
        assert_eq!(raw_newlines(b"line\r", &mut last_was_cr), b"line\r");
        assert_eq!(raw_newlines(b"\nnext", &mut last_was_cr), b"\nnext");
    }
}
