//! RFC 6455 handshake and frames on a std [`TcpStream`]. No extra runtime.

use sha1::{Digest, Sha1};
use std::io::{self, Read, Write};
use std::net::TcpStream;

const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_FRAME: u64 = 65_536;

#[derive(Debug)]
pub enum Frame {
    Text(Vec<u8>),
    Close,
    Ping(Vec<u8>),
    Pong,
    Other,
}

/// `Sec-WebSocket-Accept` for a client key (RFC 6455 §1.3).
pub fn accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.trim().as_bytes());
    hasher.update(GUID);
    let digest: [u8; 20] = hasher.finalize().into();
    b64_20(&digest)
}

pub fn is_upgrade(headers: &str) -> bool {
    let upgrade = header_has(headers, "upgrade", "websocket");
    let connection = header_csv_has(headers, "connection", "upgrade");
    upgrade && connection
}

pub fn websocket_key(headers: &str) -> Option<String> {
    header_line(headers, "sec-websocket-key")
}

pub fn websocket_version_ok(headers: &str) -> bool {
    header_line(headers, "sec-websocket-version").is_some_and(|v| v.trim() == "13")
}

pub fn write_opening(stream: &mut TcpStream, key: &str) -> io::Result<()> {
    let accept = accept_key(key);
    let header = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(header.as_bytes())
}

pub fn write_text(stream: &mut TcpStream, payload: &[u8]) -> io::Result<()> {
    write_frame(stream, 0x1, payload)
}

pub fn write_pong(stream: &mut TcpStream, payload: &[u8]) -> io::Result<()> {
    write_frame(stream, 0xA, payload)
}

pub fn write_close(stream: &mut TcpStream) -> io::Result<()> {
    write_frame(stream, 0x8, &[])
}

/// Accumulates socket bytes so a read timeout cannot drop a partial frame.
pub struct Reader {
    buf: Vec<u8>,
}

impl Reader {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Return one complete frame, or `Ok(None)` when idle or still incomplete.
    pub fn poll(&mut self, stream: &mut TcpStream) -> io::Result<Option<Frame>> {
        if let Some(frame) = take_frame(&mut self.buf)? {
            return Ok(Some(frame));
        }
        let mut tmp = [0u8; 1024];
        match stream.read(&mut tmp) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "websocket closed",
                ));
            }
            Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
            Err(err) if is_timeout(&err) => return Ok(None),
            Err(err) => return Err(err),
        }
        take_frame(&mut self.buf)
    }
}

fn is_timeout(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn take_frame(buf: &mut Vec<u8>) -> io::Result<Option<Frame>> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let opcode = buf[0] & 0x0f;
    let masked = buf[1] & 0x80 != 0;
    let mut len = u64::from(buf[1] & 0x7f);
    let mut hdr = 2usize;
    if len == 126 {
        if buf.len() < 4 {
            return Ok(None);
        }
        len = u64::from(u16::from_be_bytes([buf[2], buf[3]]));
        hdr = 4;
    } else if len == 127 {
        if buf.len() < 10 {
            return Ok(None);
        }
        let mut ext = [0u8; 8];
        ext.copy_from_slice(&buf[2..10]);
        len = u64::from_be_bytes(ext);
        hdr = 10;
    }
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mask_len = if masked { 4 } else { 0 };
    let total = hdr
        .checked_add(mask_len)
        .and_then(|n| n.checked_add(len as usize))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    if buf.len() < total {
        return Ok(None);
    }
    let frame = buf.drain(..total).collect::<Vec<_>>();
    let mut payload = frame[hdr + mask_len..].to_vec();
    if masked {
        let mask = &frame[hdr..hdr + 4];
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }
    Ok(Some(match opcode {
        0x1 => Frame::Text(payload),
        0x8 => Frame::Close,
        0x9 => Frame::Ping(payload),
        0xA => Frame::Pong,
        _ => Frame::Other,
    }))
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> io::Result<()> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | opcode);
    let n = payload.len();
    if n <= 125 {
        header.push(n as u8);
    } else if n <= 65_535 {
        header.push(126);
        header.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(n as u64).to_be_bytes());
    }
    stream.write_all(&header)?;
    stream.write_all(payload)
}

fn header_line(headers: &str, want: &str) -> Option<String> {
    for line in headers.lines() {
        let line = line.trim();
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case(want) {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn header_has(headers: &str, name: &str, want: &str) -> bool {
    header_line(headers, name).is_some_and(|v| v.eq_ignore_ascii_case(want))
}

fn header_csv_has(headers: &str, name: &str, want: &str) -> bool {
    header_line(headers, name).is_some_and(|v| {
        v.split(',')
            .any(|part| part.trim().eq_ignore_ascii_case(want))
    })
}

fn b64_20(digest: &[u8; 20]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = [0u8; 28];
    let mut i = 0;
    let mut o = 0;
    while i + 3 <= 20 {
        let n = (u32::from(digest[i]) << 16)
            | (u32::from(digest[i + 1]) << 8)
            | u32::from(digest[i + 2]);
        out[o] = T[((n >> 18) & 63) as usize];
        out[o + 1] = T[((n >> 12) & 63) as usize];
        out[o + 2] = T[((n >> 6) & 63) as usize];
        out[o + 3] = T[(n & 63) as usize];
        i += 3;
        o += 4;
    }
    let n = (u32::from(digest[18]) << 16) | (u32::from(digest[19]) << 8);
    out[24] = T[((n >> 18) & 63) as usize];
    out[25] = T[((n >> 12) & 63) as usize];
    out[26] = T[((n >> 6) & 63) as usize];
    out[27] = b'=';
    String::from_utf8(out.to_vec()).expect("base64 alphabet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_sample_accept_key() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn split_header_keeps_unread_bytes() {
        let mut buf = vec![0x81];
        assert!(take_frame(&mut buf).unwrap().is_none());
        assert_eq!(buf, vec![0x81]);
        buf.extend_from_slice(&[0x02, b'h', b'i']);
        match take_frame(&mut buf).unwrap() {
            Some(Frame::Text(payload)) => assert_eq!(payload, b"hi"),
            other => panic!("{other:?}"),
        }
        assert!(buf.is_empty());
    }
}
