//! Sealeth the school passwords we are obliged to keep.
//!
//! AES-256-GCM, a fresh nonce for every sealing, and the key drawn from the
//! environment — never from the database it protecteth. A stolen dump is
//! therefore not enough on its own.

use aes_gcm::aead::{Aead, KeyInit, OsRng, rand_core::RngCore};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};

/// The version stamped on every row, so a key may one day be retired without
/// rendering what it sealed unreadable.
pub const KEY_VERSION: i32 = 1;

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

#[derive(Clone)]
pub struct Sealer {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealer(<key withheld>)")
    }
}

/// What a sealing yieldeth: the ciphertext and the nonce it was sealed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
}

impl Sealer {
    /// From a base64 key of thirty-two bytes, as `STUNDENGLAS_KEY` holdeth it.
    pub fn from_base64(raw: &str) -> Result<Self> {
        let bytes = STANDARD.decode(raw.trim()).context("STUNDENGLAS_KEY is not valid base64")?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != KEY_LEN {
            bail!("the key must be {KEY_LEN} bytes, but {} were given", bytes.len());
        }
        let key = Key::<Aes256Gcm>::from_slice(bytes);
        Ok(Self { cipher: Aes256Gcm::new(key) })
    }

    /// A fresh key, for the operator to store and never lose.
    pub fn generate_key() -> String {
        let mut bytes = [0_u8; KEY_LEN];
        OsRng.fill_bytes(&mut bytes);
        STANDARD.encode(bytes)
    }

    pub fn seal(&self, plain: &str) -> Result<Sealed> {
        let mut nonce_bytes = [0_u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, plain.as_bytes())
            .map_err(|_| anyhow::anyhow!("sealing failed"))?;
        Ok(Sealed { ciphertext, nonce: nonce_bytes.to_vec() })
    }

    pub fn unseal(&self, sealed: &Sealed) -> Result<String> {
        if sealed.nonce.len() != NONCE_LEN {
            bail!("a nonce of {} bytes cannot be right", sealed.nonce.len());
        }
        let nonce = Nonce::from_slice(&sealed.nonce);
        let plain = self.cipher.decrypt(nonce, sealed.ciphertext.as_ref()).map_err(|_| {
            anyhow::anyhow!("unsealing failed: wrong key, or the row was tampered with")
        })?;
        String::from_utf8(plain).context("what was sealed was not text")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealer() -> Sealer {
        Sealer::from_base64(&Sealer::generate_key()).unwrap()
    }

    #[test]
    fn what_is_sealed_may_be_unsealed() {
        let s = sealer();
        for secret in ["hunter2", "", "  spaces  ", "Ümläüte und ß", "🔐 emoji"] {
            let sealed = s.seal(secret).unwrap();
            assert_eq!(s.unseal(&sealed).unwrap(), secret);
        }
    }

    #[test]
    fn the_ciphertext_does_not_betray_the_secret() {
        let s = sealer();
        let sealed = s.seal("correct-horse-battery-staple").unwrap();
        let as_text = String::from_utf8_lossy(&sealed.ciphertext);
        assert!(!as_text.contains("correct"));
        assert!(!as_text.contains("staple"));
    }

    #[test]
    fn the_same_secret_sealeth_differently_each_time() {
        let s = sealer();
        let a = s.seal("same").unwrap();
        let b = s.seal("same").unwrap();
        assert_ne!(a.nonce, b.nonce, "a nonce must never be reused");
        assert_ne!(a.ciphertext, b.ciphertext, "else equal passwords would be obvious");
        assert_eq!(s.unseal(&a).unwrap(), s.unseal(&b).unwrap());
    }

    #[test]
    fn another_key_cannot_unseal_it() {
        let sealed = sealer().seal("hunter2").unwrap();
        assert!(sealer().unseal(&sealed).is_err(), "a stolen dump must be worthless alone");
    }

    #[test]
    fn tampering_is_detected() {
        let s = sealer();
        let good = s.seal("hunter2").unwrap();

        let mut bent = good.clone();
        bent.ciphertext[0] ^= 0x01;
        assert!(s.unseal(&bent).is_err(), "a flipped bit must not pass");

        let mut moved = good.clone();
        moved.nonce[0] ^= 0x01;
        assert!(s.unseal(&moved).is_err(), "a changed nonce must not pass");

        let mut shortened = good.clone();
        shortened.ciphertext.truncate(4);
        assert!(s.unseal(&shortened).is_err());
    }

    #[test]
    fn a_nonce_of_the_wrong_size_is_refused_not_panicked_upon() {
        let s = sealer();
        let mut sealed = s.seal("hunter2").unwrap();
        sealed.nonce = vec![0; 8];
        assert!(s.unseal(&sealed).is_err());
    }

    #[test]
    fn keys_must_be_the_right_length() {
        assert!(Sealer::from_bytes(&[0; 16]).is_err());
        assert!(Sealer::from_bytes(&[0; 32]).is_ok());
        assert!(Sealer::from_base64("not base64 at all !!!").is_err());
        assert!(Sealer::from_base64(&STANDARD.encode([0_u8; 31])).is_err());
    }

    #[test]
    fn a_generated_key_is_usable_and_not_constant() {
        let a = Sealer::generate_key();
        let b = Sealer::generate_key();
        assert_ne!(a, b);
        assert!(Sealer::from_base64(&a).is_ok());
    }

    #[test]
    fn the_key_is_not_printed_by_accident() {
        let s = sealer();
        assert_eq!(format!("{s:?}"), "Sealer(<key withheld>)");
    }
}
