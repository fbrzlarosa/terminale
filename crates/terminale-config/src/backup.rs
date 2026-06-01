//! Encrypted settings import / export.
//!
//! A backup is a single, self-contained, **encrypted** file. The user's
//! settings (and, only if they explicitly opt in, their SSH credentials) are
//! serialized to JSON, then sealed with XChaCha20-Poly1305 under a key derived
//! from the user's passphrase via Argon2id. Nothing is ever written in
//! plaintext — wrong passphrase or a corrupt file fails cleanly with a clear
//! error.
//!
//! ## File layout (all little-endian where it matters)
//!
//! ```text
//! offset  size  field
//! 0       8     magic            b"TRMLBKP\x01"  (format version 1)
//! 8       16    argon2 salt      random per export
//! 24      24    xchacha nonce    random per export
//! 48      ..    ciphertext       AEAD seal of the JSON payload
//! ```
//!
//! The 48-byte header (magic ‖ salt ‖ nonce) is fed to the AEAD as associated
//! data, so tampering with the version, salt, or nonce is detected on decrypt.

use crate::Config;
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng, Payload};
use chacha20poly1305::{AeadCore, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 8-byte magic + version tag. The trailing byte is the format version; bump
/// it (and branch in [`decrypt`]) if the layout ever changes.
const MAGIC: [u8; 8] = *b"TRMLBKP\x01";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = MAGIC.len() + SALT_LEN + NONCE_LEN; // 48
const KEY_LEN: usize = 32;

// Argon2id parameters. 64 MiB memory, 3 passes, 1 lane — a sensible
// interactive-desktop cost (well above the OWASP minimum) that still finishes
// in a fraction of a second. Stored implicitly by the format version, so a
// future tuning bump requires a new MAGIC version byte.
const ARGON_M_COST_KIB: u32 = 64 * 1024;
const ARGON_T_COST: u32 = 3;
const ARGON_P_COST: u32 = 1;

/// One stored credential carried in a backup: the keychain key + its secret.
/// Only present when the user ticked "include credentials" on export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupCredential {
    /// Keychain key this secret is stored under (e.g. `ssh:<id>`).
    pub secret_id: String,
    /// The secret itself (password / passphrase).
    pub secret: String,
}

/// The decrypted contents of a backup file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupPayload {
    /// Full user configuration.
    pub config: Config,
    /// Credentials to repopulate into the OS keychain on import. Empty unless
    /// the user opted in at export time.
    #[serde(default)]
    pub credentials: Vec<BackupCredential>,
}

/// Errors from encrypting, decrypting, or validating a backup.
#[derive(Debug, Error)]
pub enum BackupError {
    /// The file is too short, or its magic/version tag doesn't match — not a
    /// terminale backup (or a newer format than this build understands).
    #[error("not a recognised terminale backup file")]
    BadFormat,
    /// Decryption failed: almost always a wrong passphrase, but also a
    /// tampered/corrupt file (the AEAD tag or associated-data check failed).
    #[error("wrong passphrase or corrupt backup")]
    Decrypt,
    /// The decrypted payload wasn't valid JSON for the current schema.
    #[error("backup payload is malformed: {0}")]
    Payload(#[from] serde_json::Error),
    /// The restored config failed validation (out-of-range field, …).
    #[error("backup config failed validation: {0}")]
    Invalid(#[from] crate::ConfigError),
    /// The passphrase was empty — refused so a backup is never effectively
    /// unencrypted.
    #[error("a non-empty passphrase is required")]
    EmptyPassphrase,
    /// Key derivation failed (should be unreachable with fixed params).
    #[error("key derivation failed")]
    Kdf,
}

/// Derive the 32-byte AEAD key from `passphrase` + `salt` via Argon2id.
fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], BackupError> {
    let params = Params::new(ARGON_M_COST_KIB, ARGON_T_COST, ARGON_P_COST, Some(KEY_LEN))
        .map_err(|_| BackupError::Kdf)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| BackupError::Kdf)?;
    Ok(key)
}

/// Encrypt `payload` under `passphrase`, returning the complete backup file
/// bytes (header + ciphertext). A fresh random salt + nonce are generated on
/// every call, so two exports of identical data produce different files.
///
/// # Errors
///
/// Returns [`BackupError::EmptyPassphrase`] for an empty passphrase, or a
/// serialization / KDF / AEAD error in the (practically unreachable) failure
/// cases.
pub fn encrypt(payload: &BackupPayload, passphrase: &str) -> Result<Vec<u8>, BackupError> {
    if passphrase.is_empty() {
        return Err(BackupError::EmptyPassphrase);
    }
    let plaintext = serde_json::to_vec(payload)?;

    // Random salt + nonce.
    let mut salt = [0u8; SALT_LEN];
    {
        use chacha20poly1305::aead::rand_core::RngCore;
        OsRng.fill_bytes(&mut salt);
    }
    let nonce: XNonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

    let key = derive_key(passphrase, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| BackupError::Kdf)?;

    // Header is the AEAD associated data, so any edit to version/salt/nonce is
    // detected on decrypt.
    let mut out = Vec::with_capacity(HEADER_LEN + plaintext.len() + 16);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(nonce.as_slice());
    let aad = out.clone(); // exactly the 48-byte header written so far

    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| BackupError::Decrypt)?;
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt and validate a backup `bytes` blob produced by [`encrypt`].
///
/// The restored [`Config`] is run through [`Config::validate`] before being
/// returned, so a corrupt-but-decryptable file still can't yield an invalid
/// config.
///
/// # Errors
///
/// - [`BackupError::BadFormat`] — not a terminale backup / unknown version.
/// - [`BackupError::Decrypt`] — wrong passphrase or tampered file.
/// - [`BackupError::Payload`] / [`BackupError::Invalid`] — decrypted but the
///   contents don't parse / validate.
pub fn decrypt(bytes: &[u8], passphrase: &str) -> Result<BackupPayload, BackupError> {
    if passphrase.is_empty() {
        return Err(BackupError::EmptyPassphrase);
    }
    if bytes.len() < HEADER_LEN || bytes[..MAGIC.len()] != MAGIC {
        return Err(BackupError::BadFormat);
    }
    let salt = &bytes[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce_bytes = &bytes[MAGIC.len() + SALT_LEN..HEADER_LEN];
    let ciphertext = &bytes[HEADER_LEN..];
    let aad = &bytes[..HEADER_LEN];

    let key = derive_key(passphrase, salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| BackupError::Kdf)?;
    let nonce = XNonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| BackupError::Decrypt)?;

    let payload: BackupPayload = serde_json::from_slice(&plaintext)?;
    payload.config.validate()?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> BackupPayload {
        let mut config = Config::default();
        config.appearance.theme = "Dracula".into();
        config.font.size = 16.0;
        config.ssh_hosts.push(crate::SshHost {
            id: "host-1".into(),
            name: "prod".into(),
            host: "10.0.0.9".into(),
            port: 2200,
            user: "deploy".into(),
            auth: crate::SshAuthMethod::Password,
            key_path: None,
        });
        BackupPayload {
            config,
            credentials: vec![BackupCredential {
                secret_id: "ssh:host-1".into(),
                secret: "s3cr3t".into(),
            }],
        }
    }

    #[test]
    fn round_trip_preserves_config_and_credentials() {
        let payload = sample_payload();
        let blob = encrypt(&payload, "correct horse battery staple").unwrap();
        let back = decrypt(&blob, "correct horse battery staple").unwrap();
        assert_eq!(back.config.appearance.theme, "Dracula");
        assert!((back.config.font.size - 16.0).abs() < f32::EPSILON);
        assert_eq!(back.config.ssh_hosts.len(), 1);
        assert_eq!(back.config.ssh_hosts[0].name, "prod");
        assert_eq!(back.credentials.len(), 1);
        assert_eq!(back.credentials[0].secret_id, "ssh:host-1");
        assert_eq!(back.credentials[0].secret, "s3cr3t");
    }

    #[test]
    fn ciphertext_is_not_plaintext() {
        // The exported blob must not leak any of the secret material or even
        // obvious config strings in the clear.
        let payload = sample_payload();
        let blob = encrypt(&payload, "pw").unwrap();
        let hay = String::from_utf8_lossy(&blob);
        for needle in ["s3cr3t", "Dracula", "deploy", "10.0.0.9"] {
            assert!(
                !hay.contains(needle),
                "encrypted backup leaked `{needle}` in the clear"
            );
        }
        // Only the magic prefix is readable.
        assert_eq!(&blob[..8], &MAGIC);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let blob = encrypt(&sample_payload(), "right").unwrap();
        let err = decrypt(&blob, "wrong").unwrap_err();
        assert!(matches!(err, BackupError::Decrypt));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let mut blob = encrypt(&sample_payload(), "pw").unwrap();
        // Flip a byte in the ciphertext body.
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(matches!(decrypt(&blob, "pw"), Err(BackupError::Decrypt)));
    }

    #[test]
    fn tampered_header_fails() {
        let mut blob = encrypt(&sample_payload(), "pw").unwrap();
        // Flip a salt byte — the header is AEAD associated data, so this is
        // detected even though it isn't part of the ciphertext.
        blob[10] ^= 0x01;
        assert!(matches!(decrypt(&blob, "pw"), Err(BackupError::Decrypt)));
    }

    #[test]
    fn bad_magic_is_rejected() {
        let bytes = b"not a backup file at all, really, trust me, padding padding";
        assert!(matches!(decrypt(bytes, "pw"), Err(BackupError::BadFormat)));
    }

    #[test]
    fn too_short_is_rejected() {
        assert!(matches!(
            decrypt(b"short", "pw"),
            Err(BackupError::BadFormat)
        ));
    }

    #[test]
    fn empty_passphrase_is_refused_both_ways() {
        assert!(matches!(
            encrypt(&sample_payload(), ""),
            Err(BackupError::EmptyPassphrase)
        ));
        let blob = encrypt(&sample_payload(), "pw").unwrap();
        assert!(matches!(
            decrypt(&blob, ""),
            Err(BackupError::EmptyPassphrase)
        ));
    }

    #[test]
    fn each_export_uses_a_fresh_salt_and_nonce() {
        // Two exports of the same data must differ (random salt + nonce), so a
        // backup file never reveals that two exports share contents.
        let p = sample_payload();
        let a = encrypt(&p, "pw").unwrap();
        let b = encrypt(&p, "pw").unwrap();
        assert_ne!(a, b);
        // Both still decrypt correctly.
        assert_eq!(decrypt(&a, "pw").unwrap().credentials.len(), 1);
        assert_eq!(decrypt(&b, "pw").unwrap().credentials.len(), 1);
    }

    #[test]
    fn export_without_credentials_carries_none() {
        let mut p = sample_payload();
        p.credentials.clear();
        let blob = encrypt(&p, "pw").unwrap();
        let back = decrypt(&blob, "pw").unwrap();
        assert!(back.credentials.is_empty());
    }
}
