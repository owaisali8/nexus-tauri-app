//! OS keychain access.
//!
//! Secrets are addressed by a `ref` name that is stored in provider config;
//! the secret itself never touches disk, logs, or the frontend.

use keyring::Entry;

const SERVICE: &str = "com.owais.essentio";

fn entry(key_ref: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, key_ref).map_err(|error| format!("keychain unavailable: {error}"))
}

pub fn set(key_ref: &str, secret: &str) -> Result<(), String> {
    entry(key_ref)?
        .set_password(secret)
        .map_err(|error| format!("could not store secret: {error}"))
}

/// `Ok(None)` when no entry exists — an absent secret is a normal state for
/// local providers, not an error.
pub fn get(key_ref: &str) -> Result<Option<String>, String> {
    match entry(key_ref)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("could not read secret: {error}")),
    }
}

pub fn delete(key_ref: &str) -> Result<(), String> {
    match entry(key_ref)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("could not delete secret: {error}")),
    }
}

/// Deterministic keychain ref for a provider id.
pub fn ref_for_provider(provider_id: &str) -> String {
    format!("provider:{provider_id}")
}
