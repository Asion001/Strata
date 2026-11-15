//! Encryption at rest for connection credentials (XChaCha20-Poly1305).

use anyhow::{Context, Result, anyhow};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;

#[derive(Clone)]
pub struct SecretBox {
    cipher: XChaCha20Poly1305,
}

impl SecretBox {
    /// Derive the key from an operator-supplied secret (any length) with
    /// BLAKE3's key derivation mode.
    pub fn from_secret(secret: &str) -> Self {
        let key = blake3::derive_key("strata 2026 connection credentials v1", secret.as_bytes());
        Self {
            cipher: XChaCha20Poly1305::new(&key.into()),
        }
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut nonce);
        let mut out = nonce.to_vec();
        let ct = self
            .cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext)
            .map_err(|_| anyhow!("encryption failed"))?;
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 24 {
            return Err(anyhow!("ciphertext too short"));
        }
        let (nonce, ct) = data.split_at(24);
        self.cipher
            .decrypt(XNonce::from_slice(nonce), ct)
            .map_err(|_| anyhow!("decryption failed (wrong STRATA_SECRET_KEY?)"))
            .context("decrypting connection credentials")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let b = super::SecretBox::from_secret("k");
        let c = b.encrypt(b"hello").unwrap();
        assert_eq!(b.decrypt(&c).unwrap(), b"hello");
        assert!(super::SecretBox::from_secret("other").decrypt(&c).is_err());
    }
}
