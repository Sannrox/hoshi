//! Unpadded base64url (RFC 4648 §5) for tokens, PKCE values and web push keys.

/// Encode without padding.
pub fn encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i];
        let b1 = data.get(i + 1).copied();
        let b2 = data.get(i + 2).copied();
        out.push(T[(b0 >> 2) as usize] as char);
        match (b1, b2) {
            (None, _) => {
                out.push(T[((b0 & 3) << 4) as usize] as char);
            }
            (Some(b1), None) => {
                out.push(T[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
                out.push(T[((b1 & 15) << 2) as usize] as char);
            }
            (Some(b1), Some(b2)) => {
                out.push(T[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
                out.push(T[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char);
                out.push(T[(b2 & 63) as usize] as char);
            }
        }
        i += 3;
    }
    out
}

/// Decode unpadded base64url; `None` for any other alphabet or a bad length.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trips_every_tail_length() {
        for len in 0..8u8 {
            let data: Vec<u8> = (0..len).map(|i| i.wrapping_mul(97)).collect();
            assert_eq!(super::decode(&super::encode(&data)), Some(data));
        }
        assert_eq!(super::decode("a"), None);
        assert_eq!(super::decode("a+b/"), None);
    }
}
