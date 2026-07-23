//! Authentication: Argon2 password hashing, stateless HMAC-signed session
//! tokens (same scheme as Scriber), and opaque alphanumeric API tokens stored
//! as SHA-256 hashes (also the Scriber scheme, so LLM tooling is consistent).

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 hash")
        .to_string()
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

fn sign(payload: &str, secret: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Create a signed session token: b64url("user_id:expiry") + "." + hex hmac.
pub fn create_session(user_id: &str, secret: &str, ttl_seconds: i64) -> String {
    let expiry = chrono::Utc::now().timestamp() + ttl_seconds;
    let payload = format!("{user_id}:{expiry}");
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes());
    format!("{encoded}.{}", sign(&payload, secret))
}

/// Verify a session token; returns the user id when valid and unexpired.
pub fn verify_session(token: &str, secret: &str) -> Option<String> {
    let (encoded, signature) = token.split_once('.')?;
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded.as_bytes())
        .ok()?;
    let payload = String::from_utf8(payload_bytes).ok()?;
    let expected = sign(&payload, secret);
    // Constant-time compare of the hex signatures.
    if expected.len() != signature.len() {
        return None;
    }
    let diff = expected
        .bytes()
        .zip(signature.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if diff != 0 {
        return None;
    }
    let (user_id, expiry) = payload.rsplit_once(':')?;
    let expiry: i64 = expiry.parse().ok()?;
    if chrono::Utc::now().timestamp() > expiry {
        return None;
    }
    Some(user_id.to_string())
}

/// 48-char alphanumeric API token (~285 bits) — chat-paste safe.
pub fn generate_api_token() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..48)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn token_prefix(token: &str) -> String {
    token.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("s3cret");
        assert!(verify_password("s3cret", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn session_roundtrip() {
        let t = create_session("u123", "key", 60);
        assert_eq!(verify_session(&t, "key").as_deref(), Some("u123"));
        assert!(verify_session(&t, "other").is_none());
        let expired = create_session("u123", "key", -1);
        assert!(verify_session(&expired, "key").is_none());
    }

    #[test]
    fn api_token_shape() {
        let t = generate_api_token();
        assert_eq!(t.len(), 48);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(hash_token(&t).len(), 64);
        assert_eq!(token_prefix(&t).len(), 8);
    }
}
