//! The `hoshi-web.pin` format, shared by `build.rs` and its tests.
//!
//! One line, `<tag> <sha256-of-zip>`, naming the hoshi-web release that a
//! `--features ui` build embeds.

/// A pinned hoshi-web release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub tag: String,
    pub sha256: String,
}

impl Pin {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut words = text.split_whitespace();
        let (Some(tag), Some(sha256), None) = (words.next(), words.next(), words.next()) else {
            return Err("pin must be one line: <tag> <sha256>".into());
        };
        let tag_ok = tag.strip_prefix('v').is_some_and(|version| {
            let parts: Vec<&str> = version.split('.').collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        });
        if !tag_ok {
            return Err(format!("pin tag {tag:?} is not vX.Y.Z"));
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("pin checksum must be 64 lowercase hex digits".into());
        }
        Ok(Self {
            tag: tag.into(),
            sha256: sha256.into(),
        })
    }

    pub fn url(&self) -> String {
        format!(
            "https://github.com/Sannrox/hoshi-web/releases/download/{tag}/hoshi-web-{tag}.zip",
            tag = self.tag
        )
    }

    /// Refuse a bundle whose SHA-256 is not the pinned one.
    pub fn verify(&self, zip: &[u8]) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        let actual: String = Sha256::digest(zip)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if actual == self.sha256 {
            Ok(())
        } else {
            Err(format!(
                "hoshi-web {} checksum mismatch: pinned {}, got {actual}",
                self.tag, self.sha256
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_pin_parses() {
        let pin = Pin::parse(include_str!("../hoshi-web.pin")).unwrap();
        assert!(
            pin.url()
                .ends_with(&format!("/{0}/hoshi-web-{0}.zip", pin.tag))
        );
    }

    #[test]
    fn checksum_mismatch_is_refused() {
        use sha2::{Digest, Sha256};
        let zip = b"bundle bytes";
        let sha: String = Sha256::digest(zip)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let pin = Pin::parse(&format!("v1.2.3 {sha}")).unwrap();
        assert!(pin.verify(zip).is_ok());
        assert!(pin.verify(b"tampered bytes").is_err());
    }

    #[test]
    fn malformed_pins_are_refused() {
        let sha = "a".repeat(64);
        for text in [
            "",
            "v1.2.3",
            &format!("1.2.3 {sha}"),
            &format!("v1.2 {sha}"),
            &format!("v1.2.3 {}", "A".repeat(64)),
            &format!("v1.2.3 {sha} extra"),
        ] {
            assert!(Pin::parse(text).is_err(), "{text:?}");
        }
    }
}
