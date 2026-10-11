//! Shareable galaxy seeds: a short checksummed code for the 64-bit generator input.
//!
//! Crockford base32 without I, L, O or U. `I`/`L` decode as 1 and `O` as 0 so
//! handwritten codes still parse. Encode always emits grouped
//! `XXXX-XXXX-XXXX-XX`. Grouped input is a code. Ungrouped all-digit input is
//! a raw `u64`, so 14-digit decimals are not decoded as base32. An ungrouped
//! 14-character string with letters is a pasted compact code. Decimal numbers
//! stay valid so existing title and harness seeds keep working. Bump
//! [`GALAXY_GENERATOR_VERSION`] when `planet_profile` or outpost placement
//! changes; old saves keep the version they were created with.

/// Generator that produced the current planet-type and outpost tables.
pub const GALAXY_GENERATOR_VERSION: u32 = 1;

/// True when this build can reproduce galaxies stored with `version`.
pub fn supports_galaxy_generator(version: u32) -> bool {
    version <= GALAXY_GENERATOR_VERSION
}

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Grouped 14-character code for `seed`, including a checksum symbol.
pub fn encode_galaxy_seed(seed: u64) -> String {
    let mut syms = [0u8; 14];
    let payload = payload_symbols(seed);
    syms[..13].copy_from_slice(&payload);
    syms[13] = checksum(&payload);
    let mut out = String::with_capacity(17);
    for (i, v) in syms.iter().enumerate() {
        if i > 0 && i % 4 == 0 {
            out.push('-');
        }
        out.push(ALPHABET[*v as usize] as char);
    }
    out
}

/// Blank is a random seed. Grouped codes, compact codes that contain a letter,
/// and decimal `u64` values parse.
pub fn parse_galaxy_seed(text: &str) -> Result<Option<u64>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let compact: String = trimmed
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != '-')
        .collect();
    // Grouped `XXXX-XXXX-XXXX-XX` is always a code. Ungrouped all-digit input is
    // a raw u64 (14-digit decimals must not be decoded as base32). Letters in an
    // ungrouped 14-character string still mean a pasted compact code.
    let grouped = trimmed.contains('-');
    let letters = compact.bytes().any(|b| b.is_ascii_alphabetic());
    if compact.len() == 14 && (grouped || letters) {
        return decode_code(&compact).map(Some);
    }
    if compact.bytes().all(|b| b.is_ascii_digit()) {
        return compact
            .parse::<u64>()
            .map(Some)
            .map_err(|_| "invalid seed".to_string());
    }
    Err("invalid seed".into())
}

fn payload_symbols(seed: u64) -> [u8; 13] {
    let mut n = seed;
    let mut out = [0u8; 13];
    for i in (0..13).rev() {
        out[i] = (n & 31) as u8;
        n >>= 5;
    }
    out
}

fn checksum(syms: &[u8; 13]) -> u8 {
    let mut acc = 0u32;
    for (i, v) in syms.iter().enumerate() {
        acc += u32::from(*v) * (i as u32 + 1);
    }
    (acc % 32) as u8
}

fn decode_code(compact: &str) -> Result<u64, String> {
    let bytes = compact.as_bytes();
    let mut syms = [0u8; 14];
    for (i, b) in bytes.iter().enumerate() {
        syms[i] = decode_char(*b).ok_or_else(|| "invalid seed".to_string())?;
    }
    let mut payload = [0u8; 13];
    payload.copy_from_slice(&syms[..13]);
    if checksum(&payload) != syms[13] {
        return Err("invalid seed".into());
    }
    // 13×5-bit groups are 65 bits; a 64-bit seed only uses the low 4 bits of the first group.
    if payload[0] > 15 {
        return Err("invalid seed".into());
    }
    let mut seed = 0u64;
    for v in payload {
        seed = (seed << 5) | u64::from(v);
    }
    Ok(seed)
}

fn decode_char(b: u8) -> Option<u8> {
    let c = match b.to_ascii_uppercase() {
        b'I' | b'L' => b'1',
        b'O' => b'0',
        other => other,
    };
    ALPHABET.iter().position(|&x| x == c).map(|i| i as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::fleets::Coord;
    use crate::outposts::generate_outposts;
    use crate::planets::{PlanetType, planet_profile};

    #[test]
    fn codes_round_trip_and_reject_typos() {
        for seed in [0, 1, 42, u64::MAX, 0xA11A_5EED_00C0_FFEE] {
            let code = encode_galaxy_seed(seed);
            assert_eq!(parse_galaxy_seed(&code).unwrap(), Some(seed), "{code}");
            let compact = code.replace('-', "");
            if compact.bytes().any(|b| b.is_ascii_alphabetic()) {
                assert_eq!(parse_galaxy_seed(&compact).unwrap(), Some(seed));
            }
            assert_eq!(parse_galaxy_seed(&code.to_lowercase()).unwrap(), Some(seed));
        }
        assert_eq!(parse_galaxy_seed("").unwrap(), None);
        assert_eq!(parse_galaxy_seed("  42  ").unwrap(), Some(42));
        assert_eq!(
            parse_galaxy_seed("12345678901234").unwrap(),
            Some(12_345_678_901_234)
        );
        assert_eq!(
            parse_galaxy_seed("10000000000001").unwrap(),
            Some(10_000_000_000_001)
        );
        let code = encode_galaxy_seed(7);
        let mut bad: Vec<char> = code.chars().collect();
        let last = bad.len() - 1;
        bad[last] = if bad[last] == '0' { '1' } else { '0' };
        let tampered: String = bad.into_iter().collect();
        assert!(parse_galaxy_seed(&tampered).is_err(), "{tampered}");
        assert!(parse_galaxy_seed("nope").is_err());
        assert!(parse_galaxy_seed("abc").is_err());
    }

    #[test]
    fn ungrouped_all_digit_codes_stay_decimal() {
        let code = encode_galaxy_seed(3);
        assert_eq!(parse_galaxy_seed(&code).unwrap(), Some(3));
        let compact = code.replace('-', "");
        assert!(
            compact.bytes().all(|b| b.is_ascii_digit()),
            "{compact} should be the all-digit case"
        );
        assert_eq!(
            parse_galaxy_seed(&compact).unwrap(),
            Some(compact.parse::<u64>().unwrap())
        );
    }

    #[test]
    fn overwide_payload_is_rejected() {
        let mut payload = [0u8; 13];
        payload[0] = 16;
        let mut compact = String::new();
        for v in payload {
            compact.push(ALPHABET[v as usize] as char);
        }
        compact.push(ALPHABET[checksum(&payload) as usize] as char);
        assert!(parse_galaxy_seed(&compact).is_err());
    }

    #[test]
    fn handwritten_confusables_decode() {
        let seed = 0x1111_0000_0000_0001;
        let code = encode_galaxy_seed(seed);
        let confusing = code.replace('1', "I").replace('0', "O");
        assert_eq!(parse_galaxy_seed(&confusing).unwrap(), Some(seed));
    }

    #[test]
    fn this_build_supports_current_and_legacy_generator() {
        assert!(supports_galaxy_generator(0));
        assert!(supports_galaxy_generator(GALAXY_GENERATOR_VERSION));
        assert!(!supports_galaxy_generator(GALAXY_GENERATOR_VERSION + 1));
    }

    #[test]
    fn golden_galaxies_match_for_fixed_codes() {
        let cases: &[(u64, PlanetType, u8)] = &[
            (0, PlanetType::Rocky, 133),
            (42, PlanetType::Rocky, 186),
            (0xDEAD_BEEF_F00D, PlanetType::Rocky, 133),
        ];
        for &(seed, kind, size) in cases {
            let code = encode_galaxy_seed(seed);
            let parsed = parse_galaxy_seed(&code).unwrap().unwrap();
            assert_eq!(parsed, seed);
            let (home_kind, home_size) =
                planet_profile(parsed, Coord::HOME, GALAXY_GENERATOR_VERSION);
            assert_eq!((home_kind, home_size), (kind, size), "seed {seed} {code}");
            let cfg = BalanceConfig::default();
            let a = generate_outposts(
                parsed,
                0,
                &cfg.outposts,
                &[Coord::HOME],
                GALAXY_GENERATOR_VERSION,
            );
            let b = generate_outposts(
                parsed,
                0,
                &cfg.outposts,
                &[Coord::HOME],
                GALAXY_GENERATOR_VERSION,
            );
            assert_eq!(a, b, "{code}");
        }
    }
}
