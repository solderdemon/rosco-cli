//! Minimal classic Kermit sender for rosco_m68k's serial bootloader.
//!
//! The protocol is deliberately isolated from CLI rendering. It implements
//! short packets, Send-Init negotiation, control and 8th-bit quoting, block
//! checks 1-3, and stop-and-wait retransmission. Long packets, windows, and
//! repeat-count compression are not advertised.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serialport::{ClearBuffer, SerialPort};

const SOH: u8 = 0x01;

#[derive(Clone, Copy, Debug)]
pub struct KermitOptions {
    pub max_retries: u32,
    pub packet_timeout: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferProgress {
    pub name: String,
    pub sent: usize,
    pub total: usize,
}

struct Params {
    maxl: usize,
    eol: u8,
    qctl: u8,
    qbin: Option<u8>,
    chkt: u8,
    npad: usize,
    padc: u8,
}

struct RxPacket {
    seq: u8,
    ptype: u8,
    data: Vec<u8>,
}

#[inline]
fn tochar(byte: u8) -> u8 {
    byte + 32
}

#[inline]
fn unchar(byte: u8) -> u8 {
    byte.wrapping_sub(32)
}

#[inline]
fn ctl(byte: u8) -> u8 {
    byte ^ 0x40
}

fn check_len(chkt: u8) -> usize {
    match chkt {
        b'2' => 2,
        b'3' => 3,
        _ => 1,
    }
}

fn crc16_kermit(bytes: impl IntoIterator<Item = u8>) -> u16 {
    let mut crc = 0_u16;
    for byte in bytes {
        let q = (crc ^ u16::from(byte)) & 0x0f;
        crc = (crc >> 4) ^ q.wrapping_mul(0x1081);
        let q = (crc ^ (u16::from(byte) >> 4)) & 0x0f;
        crc = (crc >> 4) ^ q.wrapping_mul(0x1081);
    }
    crc
}

fn build_packet(seq: u8, ptype: u8, data: &[u8], params: &Params) -> Vec<u8> {
    let clen = check_len(params.chkt);
    let len_char = tochar((data.len() + clen + 2) as u8);
    let seq_char = tochar(seq % 64);
    let mut sum = u32::from(len_char) + u32::from(seq_char) + u32::from(ptype);
    sum += data.iter().map(|byte| u32::from(*byte)).sum::<u32>();

    let mut packet = Vec::with_capacity(data.len() + clen + 5 + params.npad);
    packet.extend(std::iter::repeat_n(params.padc, params.npad));
    packet.extend([SOH, len_char, seq_char, ptype]);
    packet.extend_from_slice(data);

    match params.chkt {
        b'2' => {
            let value = sum & 0x0fff;
            packet.push(tochar(((value >> 6) & 0x3f) as u8));
            packet.push(tochar((value & 0x3f) as u8));
        }
        b'3' => {
            let crc = crc16_kermit(
                [len_char, seq_char, ptype]
                    .into_iter()
                    .chain(data.iter().copied()),
            );
            packet.push(tochar(((crc >> 12) & 0x0f) as u8));
            packet.push(tochar(((crc >> 6) & 0x3f) as u8));
            packet.push(tochar((crc & 0x3f) as u8));
        }
        _ => {
            let value = sum & 0xff;
            let check = (value + ((value & 0xc0) >> 6)) & 0x3f;
            packet.push(tochar(check as u8));
        }
    }
    packet.push(params.eol);
    packet
}

fn encode_byte(output: &mut Vec<u8>, byte: u8, params: &Params) {
    let mut current = byte;
    let mut high = false;
    if params.qbin.is_some() && current & 0x80 != 0 {
        high = true;
        current &= 0x7f;
    }
    if high {
        output.push(params.qbin.expect("qbin was checked"));
    }

    let low = current & 0x7f;
    if low < 0x20 || low == 0x7f {
        // Preserve bit 7 when rosco does not negotiate QBIN. Masking to `low`
        // here corrupts bytes such as 0x8d while still passing packet checks.
        output.extend([params.qctl, ctl(current)]);
    } else if low == params.qctl || params.qbin == Some(low) {
        output.extend([params.qctl, current]);
    } else {
        output.push(current);
    }
}

fn read_byte(
    port: &mut (impl SerialPort + ?Sized),
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<u8> {
    let mut byte = [0_u8; 1];
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("transfer cancelled");
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for response");
        }
        match port.read(&mut byte) {
            Ok(0) => {}
            Ok(_) => return Ok(byte[0]),
            Err(error) if error.kind() == ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("could not read Kermit response"),
        }
    }
}

fn read_packet(
    port: &mut (impl SerialPort + ?Sized),
    chkt: u8,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<RxPacket> {
    while read_byte(port, deadline, cancel)? != SOH {}

    let len_char = read_byte(port, deadline, cancel)?;
    let count = usize::from(unchar(len_char));
    let clen = check_len(chkt);
    if count < 2 + clen {
        bail!("malformed Kermit packet: length is too small");
    }

    let mut rest = Vec::with_capacity(count);
    while rest.len() < count {
        rest.push(read_byte(port, deadline, cancel)?);
    }
    let body = &rest[..rest.len() - clen];
    let received_check = &rest[rest.len() - clen..];
    let sum = u32::from(len_char) + body.iter().map(|byte| u32::from(*byte)).sum::<u32>();

    let valid = match chkt {
        b'2' => {
            let value = sum & 0x0fff;
            received_check
                == [
                    tochar(((value >> 6) & 0x3f) as u8),
                    tochar((value & 0x3f) as u8),
                ]
        }
        b'3' => {
            let crc = crc16_kermit(std::iter::once(len_char).chain(body.iter().copied()));
            received_check
                == [
                    tochar(((crc >> 12) & 0x0f) as u8),
                    tochar(((crc >> 6) & 0x3f) as u8),
                    tochar((crc & 0x3f) as u8),
                ]
        }
        _ => {
            let value = sum & 0xff;
            let check = (value + ((value & 0xc0) >> 6)) & 0x3f;
            received_check == [tochar(check as u8)]
        }
    };
    if !valid {
        bail!("Kermit block check mismatch");
    }

    Ok(RxPacket {
        seq: unchar(body[0]),
        ptype: body[1],
        data: body[2..].to_vec(),
    })
}

fn parse_params(data: &[u8]) -> Params {
    let get = |index: usize| data.get(index).copied();
    let maxl = get(0)
        .map(|byte| usize::from(unchar(byte)))
        .filter(|length| *length > 0)
        .unwrap_or(80)
        .min(94);
    let npad = get(2).map(|byte| usize::from(unchar(byte))).unwrap_or(0);
    let padc = get(3).map(ctl).unwrap_or(0);
    let eol = get(4)
        .map(unchar)
        .filter(|byte| *byte != 0)
        .unwrap_or(b'\r');
    let qbin = match get(6) {
        Some(byte)
            if byte != b'Y' && byte != b'N' && byte != b' ' && (33..=126).contains(&byte) =>
        {
            Some(byte)
        }
        _ => None,
    };
    let chkt = get(7)
        .filter(|byte| matches!(byte, b'1' | b'2' | b'3'))
        .unwrap_or(b'1');

    Params {
        maxl,
        eol,
        qctl: b'#',
        qbin,
        chkt,
        npad,
        padc,
    }
}

fn send_and_ack(
    port: &mut (impl SerialPort + ?Sized),
    seq: u8,
    ptype: u8,
    data: &[u8],
    params: &Params,
    cancel: &AtomicBool,
    options: KermitOptions,
) -> Result<()> {
    let packet = build_packet(seq, ptype, data, params);
    let mut last_error = anyhow!("no response");

    for _ in 0..options.max_retries.max(1) {
        if cancel.load(Ordering::Relaxed) {
            bail!("transfer cancelled");
        }
        port.write_all(&packet)
            .context("could not send Kermit packet")?;
        port.flush().context("could not flush Kermit packet")?;

        match read_packet(
            port,
            params.chkt,
            Instant::now() + options.packet_timeout,
            cancel,
        ) {
            Ok(reply) if reply.ptype == b'Y' && reply.seq == seq % 64 => return Ok(()),
            Ok(reply) if reply.ptype == b'E' => {
                bail!("remote error: {}", String::from_utf8_lossy(&reply.data));
            }
            Ok(_) => last_error = anyhow!("unexpected Kermit reply"),
            Err(error) => last_error = error,
        }
    }
    Err(last_error).context("Kermit retries exhausted")
}

fn exchange_init(
    port: &mut (impl SerialPort + ?Sized),
    cancel: &AtomicBool,
    options: KermitOptions,
) -> Result<Params> {
    let send_init_data = [
        tochar(80),
        tochar(5),
        tochar(0),
        ctl(0),
        tochar(b'\r'),
        b'#',
        b'Y',
        b'1',
        b' ',
    ];
    let init = Params {
        maxl: 94,
        eol: b'\r',
        qctl: b'#',
        qbin: None,
        chkt: b'1',
        npad: 0,
        padc: 0,
    };
    let packet = build_packet(0, b'S', &send_init_data, &init);
    let mut last_error = anyhow!("receiver did not answer Send-Init");

    for _ in 0..options.max_retries.max(1) {
        if cancel.load(Ordering::Relaxed) {
            bail!("transfer cancelled");
        }
        port.write_all(&packet)
            .context("could not send Send-Init")?;
        port.flush().context("could not flush Send-Init")?;
        match read_packet(port, b'1', Instant::now() + options.packet_timeout, cancel) {
            Ok(reply) if reply.ptype == b'Y' => return Ok(parse_params(&reply.data)),
            Ok(reply) if reply.ptype == b'E' => {
                bail!("remote error: {}", String::from_utf8_lossy(&reply.data));
            }
            Ok(_) => last_error = anyhow!("unexpected reply to Send-Init"),
            Err(error) => last_error = error,
        }
    }
    Err(last_error).context("Kermit Send-Init failed")
}

pub fn send_file(
    port: &mut (impl SerialPort + ?Sized),
    path: &Path,
    cancel: &AtomicBool,
    options: KermitOptions,
    mut progress: impl FnMut(TransferProgress),
) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "PROGRAM.BIN".into());

    let _ = port.clear(ClearBuffer::Input);
    let params = exchange_init(port, cancel, options)?;
    let total = bytes.len();
    progress(TransferProgress {
        name: name.clone(),
        sent: 0,
        total,
    });

    let mut header = Vec::new();
    for byte in name.bytes() {
        encode_byte(&mut header, byte, &params);
    }
    let budget = params
        .maxl
        .saturating_sub(check_len(params.chkt) + 2)
        .max(4);
    if header.len() > budget {
        bail!(
            "file name is too long for a Kermit short packet; rename `{name}` to {} encoded bytes or fewer",
            budget
        );
    }
    send_and_ack(port, 1, b'F', &header, &params, cancel, options)?;

    let mut seq = 2_u8;
    let mut index = 0_usize;
    while index < bytes.len() {
        if cancel.load(Ordering::Relaxed) {
            bail!("transfer cancelled");
        }
        let mut chunk = Vec::with_capacity(budget);
        while index < bytes.len() && chunk.len() + 4 <= budget {
            encode_byte(&mut chunk, bytes[index], &params);
            index += 1;
        }
        send_and_ack(port, seq, b'D', &chunk, &params, cancel, options)?;
        progress(TransferProgress {
            name: name.clone(),
            sent: index,
            total,
        });
        seq = (seq + 1) % 64;
    }

    send_and_ack(port, seq, b'Z', &[], &params, cancel, options)?;
    seq = (seq + 1) % 64;
    send_and_ack(port, seq, b'B', &[], &params, cancel, options)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(chkt: u8, qbin: Option<u8>) -> Params {
        Params {
            maxl: 94,
            eol: b'\r',
            qctl: b'#',
            qbin,
            chkt,
            npad: 0,
            padc: 0,
        }
    }

    #[test]
    fn character_helpers_round_trip() {
        for byte in 0_u8..=94 {
            assert_eq!(unchar(tochar(byte)), byte);
        }
        assert_eq!(ctl(ctl(13)), 13);
    }

    #[test]
    fn crc_matches_kermit_reference_vector() {
        assert_eq!(crc16_kermit(b"123456789".iter().copied()), 0x2189);
    }

    #[test]
    fn encodes_control_and_quote_characters() {
        let params = params(b'1', None);
        let mut output = Vec::new();
        encode_byte(&mut output, 0x00, &params);
        assert_eq!(output, [b'#', b'@']);

        output.clear();
        encode_byte(&mut output, b'#', &params);
        assert_eq!(output, [b'#', b'#']);
    }

    #[test]
    fn keeps_high_bit_when_binary_quote_is_not_negotiated() {
        let params = params(b'1', None);
        let mut output = Vec::new();
        encode_byte(&mut output, 0x8d, &params);
        assert_eq!(output, [b'#', 0xcd]);
        assert_eq!(ctl(output[1]), 0x8d);
    }

    #[test]
    fn uses_binary_quote_when_negotiated() {
        let params = params(b'1', Some(b'&'));
        let mut output = Vec::new();
        encode_byte(&mut output, 0xc1, &params);
        assert_eq!(output, [b'&', b'A']);
    }

    #[test]
    fn builds_expected_type_one_packet_length() {
        let params = params(b'1', None);
        let packet = build_packet(0, b'S', b"ABC", &params);
        assert_eq!(packet[0], SOH);
        assert_eq!(unchar(packet[1]), 6);
    }
}
