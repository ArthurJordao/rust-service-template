use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

const NONCE_LEN: usize = 12;

/// AEAD wrapper for encrypting TOTP secrets at rest. Stored blob is nonce‖ciphertext.
#[derive(Clone)]
pub struct MfaCipher {
    cipher: ChaCha20Poly1305,
}

impl MfaCipher {
    pub fn new(key: [u8; 32]) -> MfaCipher {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        MfaCipher { cipher }
    }

    pub fn encrypt(&self, plaintext: &str) -> anyhow::Result<Vec<u8>> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("mfa encrypt: {e}"))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn decrypt(&self, blob: &[u8]) -> anyhow::Result<String> {
        if blob.len() < NONCE_LEN {
            anyhow::bail!("mfa ciphertext too short");
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        let pt = self
            .cipher
            .decrypt(nonce, ct)
            .map_err(|e| anyhow::anyhow!("mfa decrypt: {e}"))?;
        Ok(String::from_utf8(pt)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_then_decrypt_roundtrips() {
        let cipher = MfaCipher::new([7u8; 32]);
        let blob = cipher.encrypt("JBSWY3DPEHPK3PXP").unwrap();
        assert_ne!(blob, b"JBSWY3DPEHPK3PXP"); // not plaintext
        assert_eq!(cipher.decrypt(&blob).unwrap(), "JBSWY3DPEHPK3PXP");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let cipher = MfaCipher::new([7u8; 32]);
        let mut blob = cipher.encrypt("secret").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xFF;
        assert!(cipher.decrypt(&blob).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let a = MfaCipher::new([1u8; 32]);
        let b = MfaCipher::new([2u8; 32]);
        let blob = a.encrypt("secret").unwrap();
        assert!(b.decrypt(&blob).is_err());
    }
}
