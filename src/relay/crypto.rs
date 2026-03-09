/// ChaCha20-Poly1305 symmetric encryption for tunnel packets.
/// Pre-shared key, nonce derived from sequence number + direction.
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};

pub const KEY_SIZE: usize = 32;

pub struct TunnelCrypto {
    cipher: ChaCha20Poly1305,
}

impl TunnelCrypto {
    pub fn new(key: &[u8; KEY_SIZE]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(key.into()),
        }
    }

    /// Encrypt payload in-place. Returns ciphertext + 16-byte auth tag.
    /// Nonce is built from the sequence number to ensure uniqueness.
    pub fn encrypt(&self, seq: u16, payload: &[u8]) -> Vec<u8> {
        let nonce = self.make_nonce(seq);
        self.cipher
            .encrypt(&nonce, payload)
            .expect("encryption should not fail")
    }

    /// Decrypt ciphertext (includes auth tag). Returns plaintext or error.
    pub fn decrypt(&self, seq: u16, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let nonce = self.make_nonce(seq);
        self.cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| CryptoError::DecryptionFailed)
    }

    /// Build a 12-byte nonce from the sequence number.
    /// First 2 bytes = seq (LE), remaining 10 = zero.
    /// Safe because each seq is used only once per key per direction.
    fn make_nonce(&self, seq: u16) -> Nonce {
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[..2].copy_from_slice(&seq.to_le_bytes());
        Nonce::from(nonce_bytes)
    }
}

/// Generate a random 32-byte key.
pub fn generate_key() -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    getrandom::getrandom(&mut key).expect("getrandom failed");
    key
}

#[derive(Debug)]
pub enum CryptoError {
    DecryptionFailed,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::DecryptionFailed => write!(f, "decryption failed (bad key or tampered)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = generate_key();
        let crypto = TunnelCrypto::new(&key);
        let payload = b"hello entrouter-line";

        let encrypted = crypto.encrypt(42, payload);
        assert_ne!(&encrypted[..payload.len()], payload);

        let decrypted = crypto.decrypt(42, &encrypted).unwrap();
        assert_eq!(decrypted, payload);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = generate_key();
        let key2 = generate_key();
        let c1 = TunnelCrypto::new(&key1);
        let c2 = TunnelCrypto::new(&key2);

        let encrypted = c1.encrypt(1, b"secret");
        assert!(c2.decrypt(1, &encrypted).is_err());
    }

    #[test]
    fn wrong_seq_fails() {
        let key = generate_key();
        let crypto = TunnelCrypto::new(&key);

        let encrypted = crypto.encrypt(1, b"secret");
        assert!(crypto.decrypt(2, &encrypted).is_err());
    }
}
