//! OS-keychain credential storage.
//!
//! SSH passwords and key passphrases are **never** written to `config.toml`.
//! Instead they live in the platform credential store — Windows Credential
//! Manager, the macOS Keychain, or the Linux Secret Service — keyed by a
//! stable host id. The config only ever holds metadata (name, host, port,
//! user, auth *type*, key path); the secret is fetched from the keychain at
//! connect time.
//!
//! All entries are stored under the service name [`SERVICE`]. The "username"
//! component of each keychain entry is the host's stable id (see
//! [`crate::SshHost::secret_id`]).

use thiserror::Error;

/// Service name every terminale secret is stored under in the OS keychain.
/// Stable across releases so existing credentials keep resolving.
pub const SERVICE: &str = "terminale";

/// Keychain id for the Anthropic Claude API key (Settings → AI).
/// Stable across releases.
pub const AI_CLAUDE_KEY_ID: &str = "ai:claude:api_key";

/// Keychain id for the OpenAI API key (Settings → AI). Stable across
/// releases.
pub const AI_OPENAI_KEY_ID: &str = "ai:openai:api_key";

/// Errors talking to the OS keychain.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The underlying platform keychain returned an error (locked keyring,
    /// permission denied, backend unavailable, …).
    #[error("keychain error: {0}")]
    Keychain(#[from] keyring::Error),
}

/// Store `secret` for `host_id` in the OS keychain, overwriting any existing
/// value. Pass an empty string only if you intend to record an empty secret;
/// prefer [`delete_secret`] to remove an entry entirely.
///
/// # Errors
///
/// Returns [`SecretError::Keychain`] when the platform credential store is
/// unavailable or rejects the write.
pub fn store_secret(host_id: &str, secret: &str) -> Result<(), SecretError> {
    let entry = keyring::Entry::new(SERVICE, host_id)?;
    entry.set_password(secret)?;
    Ok(())
}

/// Fetch the secret for `host_id` from the OS keychain.
///
/// Returns `Ok(None)` when no secret has been stored for this id (a normal,
/// expected state — e.g. the first time a password host is opened). Any other
/// keychain failure is surfaced as [`SecretError::Keychain`].
///
/// # Errors
///
/// Returns [`SecretError::Keychain`] for backend failures other than a plain
/// "entry not found".
pub fn get_secret(host_id: &str) -> Result<Option<String>, SecretError> {
    let entry = keyring::Entry::new(SERVICE, host_id)?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SecretError::Keychain(e)),
    }
}

/// Delete the secret for `host_id` from the OS keychain.
///
/// Deleting a non-existent entry is a no-op (returns `Ok(())`), so callers can
/// "ensure removed" without first checking for existence.
///
/// # Errors
///
/// Returns [`SecretError::Keychain`] for backend failures other than a plain
/// "entry not found".
pub fn delete_secret(host_id: &str) -> Result<(), SecretError> {
    let entry = keyring::Entry::new(SERVICE, host_id)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(SecretError::Keychain(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The service name must stay stable so credentials saved by older builds
    // keep resolving — a pure-data assertion that needs no live keychain.
    #[test]
    fn service_name_is_stable() {
        assert_eq!(SERVICE, "terminale");
    }

    // The live round-trip needs a real OS keychain (Credential Manager /
    // Keychain / Secret Service), which most CI sandboxes lack — gated behind
    // `--ignored` so it never breaks headless runs but can be exercised
    // locally with `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs an OS keychain"]
    fn store_get_delete_round_trip() {
        let id = format!("test-host-{}", std::process::id());
        // Clean slate.
        delete_secret(&id).unwrap();
        assert_eq!(get_secret(&id).unwrap(), None);

        store_secret(&id, "hunter2").unwrap();
        assert_eq!(get_secret(&id).unwrap().as_deref(), Some("hunter2"));

        // Overwrite.
        store_secret(&id, "correct horse").unwrap();
        assert_eq!(get_secret(&id).unwrap().as_deref(), Some("correct horse"));

        // Delete, then it's gone; deleting again is a no-op.
        delete_secret(&id).unwrap();
        assert_eq!(get_secret(&id).unwrap(), None);
        delete_secret(&id).unwrap();
    }
}
