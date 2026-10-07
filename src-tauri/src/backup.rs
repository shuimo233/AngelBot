//! Password-protected backup envelope.
//!
//! This module deliberately knows nothing about SQLite or application state.
//! The settings command remains the only owner of the export/import data
//! contract; this boundary only turns that JSON payload into an authenticated,
//! versioned portable envelope and back.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

const FORMAT: &str = "angelbot.encrypted-backup";
const VERSION: u8 = 1;
const KDF_ALGORITHM: &str = "argon2id";
const CIPHER_ALGORITHM: &str = "aes-256-gcm";
const SALT_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;
const KEY_BYTES: usize = 32;
const MEMORY_KIB: u32 = 19 * 1024;
const ITERATIONS: u32 = 2;
const PARALLELISM: u32 = 1;
const ASSOCIATED_DATA: &[u8] = b"angelbot.encrypted-backup:v1";

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedBackup {
    format: String,
    version: u8,
    kdf: KdfMetadata,
    cipher: CipherMetadata,
    ciphertext: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct KdfMetadata {
    algorithm: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CipherMetadata {
    algorithm: String,
    nonce: String,
}

/// Encrypt a canonical AngelBot export without retaining the supplied password.
pub fn encrypt_export(plaintext: &str, password: &str) -> Result<String, String> {
    validate_password(password)?;
    let mut salt = [0_u8; SALT_BYTES];
    let mut nonce = [0_u8; NONCE_BYTES];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);

    let key = derive_key(password, &salt)?;
    let ciphertext = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| "Unable to initialize backup encryption".to_string())?
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plaintext.as_bytes(),
                aad: ASSOCIATED_DATA,
            },
        )
        .map_err(|_| "Unable to encrypt backup".to_string())?;

    serde_json::to_string_pretty(&EncryptedBackup {
        format: FORMAT.to_string(),
        version: VERSION,
        kdf: KdfMetadata {
            algorithm: KDF_ALGORITHM.to_string(),
            memory_kib: MEMORY_KIB,
            iterations: ITERATIONS,
            parallelism: PARALLELISM,
            salt: BASE64.encode(salt),
        },
        cipher: CipherMetadata {
            algorithm: CIPHER_ALGORITHM.to_string(),
            nonce: BASE64.encode(nonce),
        },
        ciphertext: BASE64.encode(ciphertext),
    })
    .map_err(|error| format!("Unable to encode encrypted backup: {error}"))
}

/// Decrypt and authenticate a backup envelope. It does not deserialize or
/// persist the payload, so the caller can reuse the normal import boundary.
pub fn decrypt_export(envelope_json: &str, password: &str) -> Result<String, String> {
    validate_password(password)?;
    let envelope: EncryptedBackup = serde_json::from_str(envelope_json)
        .map_err(|_| "Invalid encrypted backup file".to_string())?;
    validate_envelope(&envelope)?;
    let salt = decode_fixed(&envelope.kdf.salt, SALT_BYTES, "salt")?;
    let nonce: [u8; NONCE_BYTES] = decode_fixed(&envelope.cipher.nonce, NONCE_BYTES, "nonce")?
        .try_into()
        .map_err(|_| "Invalid encrypted backup nonce".to_string())?;
    let ciphertext = BASE64
        .decode(&envelope.ciphertext)
        .map_err(|_| "Invalid encrypted backup ciphertext".to_string())?;
    let key = derive_key(password, &salt)?;
    let plaintext = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| "Unable to initialize backup decryption".to_string())?
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &ciphertext,
                aad: ASSOCIATED_DATA,
            },
        )
        .map_err(|_| "Incorrect backup password or modified backup file".to_string())?;
    String::from_utf8(plaintext)
        .map_err(|_| "Encrypted backup payload is not valid UTF-8".to_string())
}

fn validate_password(password: &str) -> Result<(), String> {
    if password.chars().count() < 12 {
        return Err("Backup password must contain at least 12 characters".to_string());
    }
    Ok(())
}

fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_BYTES], String> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, Some(KEY_BYTES))
        .map_err(|_| "Invalid backup key derivation parameters".to_string())?;
    let mut key = [0_u8; KEY_BYTES];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| "Unable to derive backup key".to_string())?;
    Ok(key)
}

fn validate_envelope(envelope: &EncryptedBackup) -> Result<(), String> {
    if envelope.format != FORMAT || envelope.version != VERSION {
        return Err("Unsupported encrypted backup format".to_string());
    }
    if envelope.kdf.algorithm != KDF_ALGORITHM
        || envelope.kdf.memory_kib != MEMORY_KIB
        || envelope.kdf.iterations != ITERATIONS
        || envelope.kdf.parallelism != PARALLELISM
        || envelope.cipher.algorithm != CIPHER_ALGORITHM
    {
        return Err("Unsupported encrypted backup parameters".to_string());
    }
    Ok(())
}

fn decode_fixed(value: &str, expected_len: usize, label: &str) -> Result<Vec<u8>, String> {
    let bytes = BASE64
        .decode(value)
        .map_err(|_| format!("Invalid encrypted backup {label}"))?;
    if bytes.len() != expected_len {
        return Err(format!("Invalid encrypted backup {label}"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "correct horse battery staple";

    #[test]
    fn encrypted_backup_round_trips_without_exposing_plaintext() {
        let plaintext = r#"{\"version\":1,\"memories\":[\"private\"]}"#;
        let encrypted = encrypt_export(plaintext, PASSWORD).unwrap();

        assert!(!encrypted.contains("private"));
        assert_eq!(decrypt_export(&encrypted, PASSWORD).unwrap(), plaintext);
    }

    #[test]
    fn encrypted_backup_rejects_wrong_password_and_tampering() {
        let encrypted = encrypt_export("{}", PASSWORD).unwrap();
        assert!(decrypt_export(&encrypted, "a different backup password").is_err());

        let mut tampered: serde_json::Value = serde_json::from_str(&encrypted).unwrap();
        tampered["ciphertext"] = serde_json::Value::String("AA==".to_string());
        assert!(decrypt_export(&tampered.to_string(), PASSWORD).is_err());
    }

    #[test]
    fn backup_password_is_not_optional() {
        assert!(encrypt_export("{}", "short").is_err());
    }
}
