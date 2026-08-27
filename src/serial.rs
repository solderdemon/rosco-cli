use std::io::{ErrorKind, IsTerminal, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
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

pub fn monitor(port: Box<dyn SerialPort>) -> Result<()> {
    eprintln!("Interactive UART session; press Ctrl-C to exit.");

    let mut serial_writer = port
        .try_clone()
        .context("could not clone serial port for interactive input")?;
    let mut serial_reader = port;

    let raw_guard = RawModeGuard::enter()?;
    let is_raw = raw_guard.active;

    let running = Arc::new(AtomicBool::new(true));
    let running_writer = running.clone();

    let stdin_thread = std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0_u8; 512];

        while running_writer.load(Ordering::Relaxed) {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    for &byte in &buffer[..count] {
                        if byte == 0x03 || byte == 0x1d {
                            running_writer.store(false, Ordering::SeqCst);
                            return;
                        }
                    }

                    if serial_writer.write_all(&buffer[..count]).is_err() {
                        break;
                    }
                    let _ = serial_writer.flush();
                }
                Err(_) => break,
            }
        }
        running_writer.store(false, Ordering::SeqCst);
    });

    let mut stdout = std::io::stdout().lock();
    let mut buffer = [0_u8; 4096];
    let mut last_was_cr = false;

    while running.load(Ordering::SeqCst) {
        match serial_reader.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                if is_raw {
                    for &byte in &buffer[..count] {
                        if byte == b'\n' && !last_was_cr {
                            let _ = stdout.write_all(b"\r\n");
                        } else {
                            let _ = stdout.write_all(&[byte]);
                        }
                        last_was_cr = byte == b'\r';
                    }
                } else {
                    let _ = stdout.write_all(&buffer[..count]);
                }
                let _ = stdout.flush();
            }
            Err(error) if error.kind() == ErrorKind::TimedOut => {}
            Err(error) => {
                running.store(false, Ordering::SeqCst);
                drop(raw_guard);
                return Err(error).context("UART read failed");
            }
        }
    }

    drop(raw_guard);
    eprintln!("\r\nUART session closed.");
    let _ = stdin_thread;

    Ok(())
}
