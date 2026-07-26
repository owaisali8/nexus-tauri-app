//! OS keychain access.
//!
//! Secrets are addressed by a `ref` name that is stored in provider config;
//! the secret itself never touches disk, logs, or the frontend.

use keyring::Entry;

const SERVICE: &str = "com.owais.nexus";

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per run so concurrent or repeated runs cannot collide.
    fn test_ref() -> String {
        format!("test:{}", uuid::Uuid::new_v4())
    }

    /// The keychain is a real OS service and may be unavailable (headless CI,
    /// locked login keyring). Skip rather than fail spuriously in that case —
    /// a missing backend is an environment fact, not a defect in this code.
    fn keychain_available() -> bool {
        let probe = test_ref();
        match set(&probe, "probe") {
            Ok(()) => {
                let _ = delete(&probe);
                true
            }
            Err(error) => {
                eprintln!("skipping keychain test: {error}");
                false
            }
        }
    }

    #[test]
    fn secret_round_trips_through_the_os_keychain() {
        if !keychain_available() {
            return;
        }

        let key_ref = test_ref();
        // Not a real credential — this only proves the storage path works.
        let secret = "sk-test-not-a-real-key";

        set(&key_ref, secret).unwrap();
        assert_eq!(get(&key_ref).unwrap().as_deref(), Some(secret));

        delete(&key_ref).unwrap();
        assert!(
            get(&key_ref).unwrap().is_none(),
            "secret should be gone after delete"
        );
    }

    #[test]
    fn missing_secret_is_none_not_an_error() {
        if !keychain_available() {
            return;
        }
        assert!(get(&test_ref()).unwrap().is_none());
    }

    #[test]
    fn deleting_a_missing_secret_succeeds() {
        if !keychain_available() {
            return;
        }
        // Delete is used on provider removal, which must not fail just
        // because the provider never had a key.
        delete(&test_ref()).unwrap();
    }

    #[test]
    fn overwriting_replaces_the_stored_value() {
        if !keychain_available() {
            return;
        }

        let key_ref = test_ref();
        set(&key_ref, "first").unwrap();
        set(&key_ref, "second").unwrap();
        assert_eq!(get(&key_ref).unwrap().as_deref(), Some("second"));
        delete(&key_ref).unwrap();
    }

    #[test]
    fn provider_refs_are_namespaced_and_distinct() {
        assert_eq!(ref_for_provider("openai"), "provider:openai");
        assert_ne!(ref_for_provider("openai"), ref_for_provider("deepseek"));
    }
}
