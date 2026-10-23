//! One-time pairing codes for `hoshi serve` (ADR 0016).
//!
//! The operator mints a code; one browser exchanges it once for a device
//! session. Codes live in memory only, so a restart drops unused ones while
//! paired sessions (in the sessions file) survive.

use std::collections::HashMap;
use std::net::IpAddr;

/// Characters in a code.
pub const CODE_LEN: usize = 8;
/// A code works this long after it was minted.
pub const CODE_TTL_SECS: u64 = 10 * 60;
/// No 0/O, 1/I/L: a code can be typed from a phone screen when a camera fails.
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
/// Failed redeems allowed per client address inside [`FAILURE_WINDOW_SECS`].
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW_SECS: u64 = 10 * 60;
/// Unused codes kept at once; minting more drops the oldest.
const MAX_CODES: usize = 16;
/// Addresses with recent failures remembered at once; the stalest goes first.
const MAX_TRACKED: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Too many wrong codes from this address lately.
    Limited,
    /// Unknown, used or expired code.
    Invalid,
}

/// A minted code; [`Codes::redeem`] hands it back so a failed pairing can
/// [`Codes::restore`] it unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub code: String,
    pub label: Option<String>,
    pub expires_at: u64,
}

/// Live codes and recent failures per client address.
#[derive(Debug, Clone, Default)]
pub struct Codes {
    live: Vec<Code>,
    failures: HashMap<Option<IpAddr>, Vec<u64>>,
}

impl Codes {
    /// A fresh code and its expiry.
    pub fn mint(&mut self, label: Option<String>, now: u64) -> std::io::Result<(String, u64)> {
        let mut code = String::with_capacity(CODE_LEN);
        while code.len() < CODE_LEN {
            let mut byte = [0u8; 1];
            getrandom::getrandom(&mut byte)
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            // Reject the tail so every character is equally likely.
            let limit = (256 / ALPHABET.len() * ALPHABET.len()) as u8;
            if byte[0] < limit {
                code.push(ALPHABET[usize::from(byte[0]) % ALPHABET.len()] as char);
            }
        }
        self.live.retain(|known| known.expires_at > now);
        if self.live.len() >= MAX_CODES {
            self.live.remove(0);
        }
        let expires_at = now + CODE_TTL_SECS;
        self.live.push(Code {
            code: code.clone(),
            label,
            expires_at,
        });
        Ok((code, expires_at))
    }

    /// Consume `presented`. Case, spaces and dashes are ignored. Each wrong
    /// code counts against the client's address.
    pub fn redeem(
        &mut self,
        presented: &str,
        peer: Option<IpAddr>,
        now: u64,
    ) -> Result<Code, Refused> {
        self.failures.retain(|_, times| {
            times.retain(|at| now.saturating_sub(*at) < FAILURE_WINDOW_SECS);
            !times.is_empty()
        });
        if self.failures.len() >= MAX_TRACKED && !self.failures.contains_key(&peer) {
            let stalest = self
                .failures
                .iter()
                .min_by_key(|(_, times)| times.last().copied().unwrap_or(0))
                .map(|(addr, _)| *addr);
            if let Some(addr) = stalest {
                self.failures.remove(&addr);
            }
        }
        let failures = self.failures.entry(peer).or_default();
        if failures.len() >= MAX_FAILURES {
            return Err(Refused::Limited);
        }
        let wanted: String = presented
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .map(|c| c.to_ascii_uppercase())
            .collect();
        self.live.retain(|known| known.expires_at > now);
        let found = self
            .live
            .iter()
            .position(|known| crate::serve::eq_secret(known.code.as_bytes(), wanted.as_bytes()));
        match found {
            Some(index) => {
                if failures.is_empty() {
                    self.failures.remove(&peer);
                }
                Ok(self.live.remove(index))
            }
            None => {
                failures.push(now);
                Err(Refused::Invalid)
            }
        }
    }

    /// Put a redeemed code back when its session could not be stored, so the
    /// operator does not have to mint another.
    pub fn restore(&mut self, code: Code) {
        self.live.push(code);
    }
}

/// `XXXX-XXXX`, easier to read aloud and type.
pub fn display(code: &str) -> String {
    let (head, tail) = code.split_at(code.len().min(4));
    format!("{head}-{tail}")
}

/// A QR code for `text`, drawn with half-block characters for a terminal.
pub fn qr(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_pairs_once_and_expires() {
        let mut codes = Codes::default();
        let (code, expires_at) = codes.mint(Some("phone".into()), 1_000).unwrap();
        assert_eq!(code.len(), CODE_LEN);
        assert!(code.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
        assert_eq!(expires_at, 1_000 + CODE_TTL_SECS);
        let typed = display(&code).to_lowercase();
        let spent = codes.redeem(&typed, None, 1_001).unwrap();
        assert_eq!(
            (spent.code.as_str(), spent.label.as_deref()),
            (code.as_str(), Some("phone"))
        );
        assert_eq!(codes.redeem(&code, None, 1_002), Err(Refused::Invalid));

        let (late, _) = codes.mint(None, 2_000).unwrap();
        assert_eq!(
            codes.redeem(&late, None, 2_000 + CODE_TTL_SECS),
            Err(Refused::Invalid)
        );
    }

    #[test]
    fn wrong_codes_hit_the_limit_per_address() {
        let mut codes = Codes::default();
        let (code, _) = codes.mint(None, 0).unwrap();
        let attacker = Some("192.0.2.9".parse().unwrap());
        for _ in 0..MAX_FAILURES {
            assert_eq!(
                codes.redeem("WRONG000", attacker, 10),
                Err(Refused::Invalid)
            );
        }
        assert_eq!(codes.redeem(&code, attacker, 11), Err(Refused::Limited));
        // Another address is unaffected, and the window passes.
        let phone = Some("192.0.2.7".parse().unwrap());
        assert!(codes.redeem(&code, phone, 12).is_ok());
        let (next, _) = codes.mint(None, 20).unwrap();
        assert!(
            codes
                .redeem(&next, attacker, 10 + FAILURE_WINDOW_SECS)
                .is_ok()
        );
    }

    #[test]
    fn failure_memory_is_pruned_and_bounded() {
        let mut codes = Codes::default();
        for n in 0..(MAX_TRACKED as u32 + 10) {
            let addr = Some(IpAddr::from(n.to_be_bytes()));
            let _ = codes.redeem("WRONG000", addr, 0);
        }
        assert_eq!(codes.failures.len(), MAX_TRACKED);
        let _ = codes.redeem("WRONG000", None, FAILURE_WINDOW_SECS);
        assert_eq!(codes.failures.len(), 1);
    }

    #[test]
    fn qr_renders_the_link() {
        let art = qr("http://192.0.2.7:8787/ui/#pair=ABCD2345").unwrap();
        assert!(art.lines().count() > 10);
        assert!(art.contains('█'));
    }
}
