//! OSC 52 clipboard write. No-op when stdout is not a tty.

use std::io::{IsTerminal, Write};

pub fn copy_to_clipboard(text: &str) {
    let mut out = std::io::stdout();
    if !out.is_terminal() {
        return;
    }
    let _ = write!(out, "\x1b]52;c;{}\x07", base64_encode(text.as_bytes()));
    let _ = out.flush();
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes.get(i + 1).copied().unwrap_or(0);
        let b2 = bytes.get(i + 2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < bytes.len() {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_encodes_seed_text() {
        assert_eq!(base64_encode(b"hi"), "aGk=");
    }
}
