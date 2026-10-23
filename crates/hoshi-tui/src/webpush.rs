//! Web push message encryption (RFC 8291, `aes128gcm`) and VAPID (RFC 8292).
//!
//! Pure functions over keys and bytes; [`crate::push`] owns delivery.

use aes_gcm::Aes128Gcm;
use aes_gcm::aead::{Aead, KeyInit};
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use sha2::Sha256;

/// One record carries the whole message; push services accept 4096 bytes.
const RECORD_SIZE: u32 = 4096;
/// Salt, record size, key id length, uncompressed sender key.
const HEADER_LEN: usize = 16 + 4 + 1 + 65;
/// VAPID tokens stay valid for this long; RFC 8292 caps them at 24 hours.
const TOKEN_SECS: u64 = 12 * 3600;

/// Operator VAPID key pair, parsed from the base64url private scalar.
pub struct Vapid {
    key: SigningKey,
}

impl Vapid {
    pub fn from_private(encoded: &str) -> Option<Self> {
        let bytes = crate::b64url::decode(encoded.trim())?;
        let key = SigningKey::from_slice(&bytes).ok()?;
        Some(Self { key })
    }

    /// Uncompressed public key as base64url, the browser's `applicationServerKey`.
    pub fn public_key(&self) -> String {
        let point = self.key.verifying_key().to_encoded_point(false);
        crate::b64url::encode(point.as_bytes())
    }

    /// `Authorization` header value for a push service endpoint.
    pub fn authorization(&self, endpoint: &str, subject: &str, now: u64) -> Option<String> {
        let claims = serde_json::json!({
            "aud": origin(endpoint)?,
            "exp": now + TOKEN_SECS,
            "sub": subject,
        });
        let signing_input = format!(
            "{}.{}",
            crate::b64url::encode(br#"{"typ":"JWT","alg":"ES256"}"#),
            crate::b64url::encode(claims.to_string().as_bytes())
        );
        let signature: Signature = self.key.sign(signing_input.as_bytes());
        let jwt = format!(
            "{signing_input}.{}",
            crate::b64url::encode(&signature.to_bytes())
        );
        Some(format!("vapid t={jwt}, k={}", self.public_key()))
    }
}

/// `scheme://host[:port]` of an `https://` URL; the VAPID audience.
fn origin(endpoint: &str) -> Option<String> {
    let rest = endpoint.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| format!("https://{authority}"))
}

/// Browser subscription keys (`keys.p256dh`, `keys.auth`) after validation.
pub struct Receiver {
    public: PublicKey,
    auth: [u8; 16],
}

impl Receiver {
    pub fn parse(p256dh: &str, auth: &str) -> Option<Self> {
        let public = PublicKey::from_sec1_bytes(&crate::b64url::decode(p256dh)?).ok()?;
        let auth = crate::b64url::decode(auth)?.try_into().ok()?;
        Some(Self { public, auth })
    }
}

/// Encrypt `plaintext` for `to` with a fresh sender key and salt.
pub fn seal(to: &Receiver, plaintext: &[u8]) -> Option<Vec<u8>> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).ok()?;
    let sender = loop {
        let mut scalar = [0u8; 32];
        getrandom::getrandom(&mut scalar).ok()?;
        if let Ok(key) = SecretKey::from_slice(&scalar) {
            break key;
        }
    };
    encrypt(to, plaintext, salt, &sender)
}

/// RFC 8291 §3 with a given salt and sender key, so test vectors can pin both.
fn encrypt(to: &Receiver, plaintext: &[u8], salt: [u8; 16], sender: &SecretKey) -> Option<Vec<u8>> {
    if HEADER_LEN + plaintext.len() + 1 + 16 > RECORD_SIZE as usize {
        return None;
    }
    let sender_public = sender.public_key().to_encoded_point(false);
    let (cek, nonce) = content_keys(
        sender,
        &to.public,
        sender_public.as_bytes(),
        &to.auth,
        &salt,
        false,
    )?;
    let mut record = plaintext.to_vec();
    record.push(2);
    let sealed = Aes128Gcm::new(&cek.into())
        .encrypt(&nonce.into(), record.as_slice())
        .ok()?;
    let mut out = Vec::with_capacity(HEADER_LEN + sealed.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(65);
    out.extend_from_slice(sender_public.as_bytes());
    out.extend_from_slice(&sealed);
    Some(out)
}

/// Content encryption key and nonce. `own` is our key; `peer` is the other side.
/// `receiving` flips the key-info order so the subscriber side can decrypt in tests.
fn content_keys(
    own: &SecretKey,
    peer: &PublicKey,
    sender_public: &[u8],
    auth: &[u8; 16],
    salt: &[u8; 16],
    receiving: bool,
) -> Option<([u8; 16], [u8; 12])> {
    let shared = p256::ecdh::diffie_hellman(own.to_nonzero_scalar(), peer.as_affine());
    let receiver_public = if receiving {
        own.public_key().to_encoded_point(false)
    } else {
        peer.to_encoded_point(false)
    };
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(receiver_public.as_bytes());
    info.extend_from_slice(sender_public);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&info, &mut ikm)
        .ok()?;
    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .ok()?;
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).ok()?;
    Some((cek, nonce))
}

/// Subscriber-side decryption, used by tests to prove a payload opens.
#[cfg(test)]
pub fn open(receiver: &SecretKey, auth: &[u8; 16], body: &[u8]) -> Option<Vec<u8>> {
    let salt: [u8; 16] = body.get(..16)?.try_into().ok()?;
    let sender_public = body.get(21..HEADER_LEN)?;
    let sender = PublicKey::from_sec1_bytes(sender_public).ok()?;
    let (cek, nonce) = content_keys(receiver, &sender, sender_public, auth, &salt, true)?;
    let mut record = Aes128Gcm::new(&cek.into())
        .decrypt(&nonce.into(), body.get(HEADER_LEN..)?)
        .ok()?;
    (record.pop()? == 2).then_some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;

    fn b64(text: &str) -> Vec<u8> {
        crate::b64url::decode(text).expect("base64url")
    }

    // RFC 8291 Section 5 and Appendix A.
    const PLAINTEXT: &[u8] = b"When I grow up, I want to be a watermelon";
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    #[test]
    fn rfc8291_vector_encrypts_and_decrypts() {
        let receiver = Receiver::parse(UA_PUBLIC, AUTH).expect("receiver");
        let sender = SecretKey::from_slice(&b64(AS_PRIVATE)).unwrap();
        let salt: [u8; 16] = b64(SALT).try_into().unwrap();
        let body = encrypt(&receiver, PLAINTEXT, salt, &sender).expect("encrypt");
        assert_eq!(crate::b64url::encode(&body), BODY);

        let ua = SecretKey::from_slice(&b64(UA_PRIVATE)).unwrap();
        let auth: [u8; 16] = b64(AUTH).try_into().unwrap();
        assert_eq!(open(&ua, &auth, &b64(BODY)).as_deref(), Some(PLAINTEXT));
    }

    #[test]
    fn sealed_payload_opens_only_with_the_subscriber_keys() {
        let ua = SecretKey::from_slice(&b64(UA_PRIVATE)).unwrap();
        let receiver = Receiver::parse(UA_PUBLIC, AUTH).unwrap();
        let body = seal(&receiver, br#"{"kind":"raid"}"#).expect("seal");
        let auth: [u8; 16] = b64(AUTH).try_into().unwrap();
        assert_eq!(
            open(&ua, &auth, &body).as_deref(),
            Some(&br#"{"kind":"raid"}"#[..])
        );
        let mut wrong = auth;
        wrong[0] ^= 1;
        assert!(open(&ua, &wrong, &body).is_none());
        assert!(seal(&receiver, &[0u8; 4096]).is_none());
    }

    #[test]
    fn subscription_keys_are_validated() {
        assert!(Receiver::parse(UA_PUBLIC, AUTH).is_some());
        assert!(Receiver::parse(UA_PUBLIC, "BTBZMqHH6r4Tts7J").is_none());
        assert!(Receiver::parse("BCVxsr7N", AUTH).is_none());
    }

    #[test]
    fn vapid_header_signs_the_endpoint_origin() {
        let vapid = Vapid::from_private(AS_PRIVATE).expect("vapid");
        assert_eq!(
            vapid.public_key(),
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
        let header = vapid
            .authorization(
                "https://push.example.net:8443/send/abc?x=1",
                "mailto:ops@example.net",
                1_000,
            )
            .expect("header");
        let (token, key) = header
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .expect("vapid scheme");
        assert_eq!(key, vapid.public_key());
        let (signing_input, signature) = token.rsplit_once('.').unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&b64(signing_input.split_once('.').unwrap().1)).unwrap();
        assert_eq!(claims["aud"], "https://push.example.net:8443");
        assert_eq!(claims["exp"], 1_000 + TOKEN_SECS);
        assert_eq!(claims["sub"], "mailto:ops@example.net");
        let verifying = VerifyingKey::from_sec1_bytes(&b64(key)).unwrap();
        let signature = Signature::from_slice(&b64(signature)).unwrap();
        assert!(
            verifying
                .verify(signing_input.as_bytes(), &signature)
                .is_ok()
        );
        assert!(
            vapid
                .authorization("http://push.example.net/x", "mailto:a@b", 0)
                .is_none()
        );
        assert!(Vapid::from_private("not a key").is_none());
    }
}
