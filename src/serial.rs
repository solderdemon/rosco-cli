use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use serialport::{DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits};

#[derive(Clone, Debug)]
pub struct PortSummary {
    pub name: String,
    pub kind: String,
}

pub fn list_ports() -> Result<Vec<PortSummary>> {
    let ports = serialport::available_ports().context("could not enumerate serial ports")?;
    Ok(ports
        .into_iter()
        .map(|port| {
            let kind = match port.port_type {
                SerialPortType::UsbPort(usb) => {
                    format!("USB {:04x}:{:04x}", usb.vid, usb.pid)
                }
                SerialPortType::BluetoothPort => "Bluetooth".into(),
                SerialPortType::PciPort => "PCI".into(),
                SerialPortType::Unknown => "Serial".into(),
            };
            PortSummary {
                name: port.port_name,
                kind,
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

pub fn monitor(mut port: Box<dyn SerialPort>) -> Result<()> {
    eprintln!("Monitoring UART; press Ctrl-C to stop.");
    let mut stdout = std::io::stdout().lock();
    let mut buffer = [0_u8; 4096];

    loop {
        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                stdout.write_all(&buffer[..count])?;
                stdout.flush()?;
            }
            Err(error) if error.kind() == ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("UART read failed"),
        }
    }
}
