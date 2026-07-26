//! One-time move of data from the app's previous identity.
//!
//! Tauri derives the app data directory from the bundle identifier, and the
//! keychain service name is that identifier too. Renaming the app therefore
//! points it at an empty directory and an empty keychain, leaving the user's
//! conversations, documents and API keys intact but invisible.

use std::path::{Path, PathBuf};

/// Identifier the app shipped under before the rename.
const PREVIOUS_IDENTIFIER: &str = "com.owais.essentio";

/// Files and directories worth carrying over.
///
/// An explicit list rather than "everything": the old build also left
/// `essentio-state.json` from a prototype that nothing reads any more, and
/// copying junk forward makes it permanent.
const CARRY_OVER: &[&str] = &[
    "workspace.sqlite3",
    "workspace.sqlite3-wal",
    "workspace.sqlite3-shm",
    "providers.json",
    "mcp.json",
    "notes",
];

/// Where the previous version kept its data.
fn previous_data_dir(current: &Path) -> Option<PathBuf> {
    // Siblings under the same roaming/app-data root.
    let parent = current.parent()?;
    let candidate = parent.join(PREVIOUS_IDENTIFIER);
    candidate.is_dir().then_some(candidate)
}

/// Move data from the previous identity into `current`, once.
///
/// Copies rather than renames, so a failure part-way leaves the old data
/// untouched and the user can retry or recover by hand. Returns the names
/// carried over.
pub fn adopt_previous_data(current: &Path) -> Vec<String> {
    let Some(previous) = previous_data_dir(current) else {
        return Vec::new();
    };

    // Only adopt into a fresh install. A database already here means the user
    // has used the renamed app, and overwriting it would destroy newer work.
    if current.join("workspace.sqlite3").exists() {
        return Vec::new();
    }

    let mut adopted = Vec::new();

    for name in CARRY_OVER {
        let source = previous.join(name);
        if !source.exists() {
            continue;
        }

        let destination = current.join(name);
        let result = if source.is_dir() {
            copy_dir(&source, &destination)
        } else {
            std::fs::copy(&source, &destination).map(|_| ())
        };

        match result {
            Ok(()) => adopted.push((*name).to_string()),
            Err(error) => {
                tracing::warn!(file = name, %error, "could not carry over previous data")
            }
        }
    }

    if !adopted.is_empty() {
        tracing::info!(
            from = %previous.display(),
            files = ?adopted,
            "adopted data from the previous app identity"
        );
    }

    adopted
}

fn copy_dir(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Copy stored secrets from the previous keychain service.
///
/// Keychain entries are addressed by (service, name), and the service is the
/// bundle identifier. Without this the user re-enters every API key.
///
/// Returns the refs successfully carried over. The old entries are left in
/// place: deleting a credential is not something to do on the user's behalf
/// as a side effect of a rename.
pub fn adopt_previous_secrets(key_refs: &[String]) -> Vec<String> {
    let mut adopted = Vec::new();

    for key_ref in key_refs {
        let Ok(previous) = keyring::Entry::new(PREVIOUS_IDENTIFIER, key_ref) else {
            continue;
        };

        match previous.get_password() {
            Ok(secret) => {
                if crate::secrets::set(key_ref, &secret).is_ok() {
                    adopted.push(key_ref.clone());
                }
            }
            // No entry under the old service, or the keychain is unavailable.
            Err(_) => continue,
        }
    }

    if !adopted.is_empty() {
        tracing::info!(
            count = adopted.len(),
            "adopted secrets from the previous identity"
        );
    }

    adopted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("nexus-migrate-{label}-{}", uuid::Uuid::new_v4()));
        path
    }

    #[test]
    fn nothing_happens_without_a_previous_directory() {
        let root = temp_root("absent");
        let current = root.join("com.owais.nexus");
        std::fs::create_dir_all(&current).unwrap();

        assert!(adopt_previous_data(&current).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn data_is_carried_over_from_the_previous_identity() {
        let root = temp_root("carry");
        let previous = root.join(PREVIOUS_IDENTIFIER);
        let current = root.join("com.owais.nexus");
        std::fs::create_dir_all(&previous).unwrap();
        std::fs::create_dir_all(current.join("placeholder")).unwrap();

        std::fs::write(previous.join("workspace.sqlite3"), b"db").unwrap();
        std::fs::write(previous.join("providers.json"), b"[]").unwrap();
        std::fs::create_dir_all(previous.join("notes")).unwrap();
        std::fs::write(previous.join("notes").join("a.md"), b"note").unwrap();
        // Prototype leftover that nothing reads; must not follow.
        std::fs::write(previous.join("essentio-state.json"), b"{}").unwrap();

        let adopted = adopt_previous_data(&current);

        assert!(adopted.contains(&"workspace.sqlite3".to_string()));
        assert_eq!(
            std::fs::read(current.join("workspace.sqlite3")).unwrap(),
            b"db"
        );
        assert_eq!(
            std::fs::read(current.join("notes").join("a.md")).unwrap(),
            b"note"
        );
        assert!(
            !current.join("essentio-state.json").exists(),
            "obsolete files must not be carried forward"
        );
        // The source is left intact so a bad migration is recoverable.
        assert!(previous.join("workspace.sqlite3").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    /// Adopting over live data would destroy work done since the rename.
    #[test]
    fn an_existing_database_is_never_overwritten() {
        let root = temp_root("occupied");
        let previous = root.join(PREVIOUS_IDENTIFIER);
        let current = root.join("com.owais.nexus");
        std::fs::create_dir_all(&previous).unwrap();
        std::fs::create_dir_all(&current).unwrap();

        std::fs::write(previous.join("workspace.sqlite3"), b"old").unwrap();
        std::fs::write(current.join("workspace.sqlite3"), b"new").unwrap();

        assert!(adopt_previous_data(&current).is_empty());
        assert_eq!(
            std::fs::read(current.join("workspace.sqlite3")).unwrap(),
            b"new"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
