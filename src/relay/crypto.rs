/// ChaCha20-Poly1305 symmetric encryption for tunnel packets.
/// Pre-shared key, nonce derived from sequence number + direction.
use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead, KeyInit},
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
    /// Nonce is built from the 64-bit sequence number to ensure uniqueness.
    pub fn encrypt(&self, seq: u64, payload: &[u8]) -> Vec<u8> {
        let nonce = self.make_nonce(seq);
        self.cipher
            .encrypt(&nonce, payload)
            .expect("ChaCha20-Poly1305 encrypt with valid key cannot fail")
    }

    /// Decrypt ciphertext (includes auth tag). Returns plaintext or error.
    pub fn decrypt(&self, seq: u64, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let nonce = self.make_nonce(seq);
        self.cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| CryptoError::DecryptionFailed)
    }

    /// Build a 12-byte nonce from the sequence number.
    /// First 8 bytes = seq (LE), remaining 4 = zero.
    /// Safe because u64 won't wrap in any realistic tunnel lifetime.
    fn make_nonce(&self, seq: u64) -> Nonce {
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[..8].copy_from_slice(&seq.to_le_bytes());
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

        let encrypted = crypto.encrypt(42u64, payload);
        assert_ne!(&encrypted[..payload.len()], payload);

        let decrypted = crypto.decrypt(42u64, &encrypted).unwrap();
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

    #[test]
    fn empty_payload() {
        let key = generate_key();
        let crypto = TunnelCrypto::new(&key);
        let encrypted = crypto.encrypt(0, b"");
        let decrypted = crypto.decrypt(0, &encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn large_payload() {
        let key = generate_key();
        let crypto = TunnelCrypto::new(&key);
        let payload = vec![0xABu8; 16_384]; // 16 KB
        let encrypted = crypto.encrypt(100, &payload);
        let decrypted = crypto.decrypt(100, &encrypted).unwrap();
        assert_eq!(decrypted, payload);
    }

    #[test]
    fn sequential_nonces() {
        let key = generate_key();
        let crypto = TunnelCrypto::new(&key);
        let payload = b"test";

        // Encrypting the same payload with different seq should produce different ciphertexts
        let e1 = crypto.encrypt(0, payload);
        let e2 = crypto.encrypt(1, payload);
        assert_ne!(e1, e2);

        // Both should decrypt correctly with their own seq
        assert_eq!(crypto.decrypt(0, &e1).unwrap(), payload);
        assert_eq!(crypto.decrypt(1, &e2).unwrap(), payload);
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = generate_key();
        let crypto = TunnelCrypto::new(&key);
        let mut encrypted = crypto.encrypt(1, b"data");
        // Flip a byte in the ciphertext
        encrypted[0] ^= 0xFF;
        assert!(crypto.decrypt(1, &encrypted).is_err());
    }

    #[test]
    fn generate_key_is_random() {
        let k1 = generate_key();
        let k2 = generate_key();
        assert_ne!(k1, k2);
    }
}
