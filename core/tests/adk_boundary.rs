//! Enforces the engine boundary from the architecture doc:
//! no `adk_*` reference may exist outside `core/src/engine/adk/`.
//!
//! This is the guarantee that makes swapping engines cheap. It is a test
//! rather than a convention because conventions decay silently.

use std::{fs, path::Path};

/// Directory permitted to reference ADK.
const ALLOWED_DIR: &str = "src/engine/adk";

fn collect_rust_files(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn adk_types_do_not_leak_outside_the_adk_engine() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let allowed = crate_root.join(ALLOWED_DIR);

    let mut files = Vec::new();
    collect_rust_files(&crate_root.join("src"), &mut files);
    assert!(!files.is_empty(), "found no Rust sources to scan");

    let mut violations = Vec::new();

    for file in files {
        if file.starts_with(&allowed) {
            continue;
        }

        let Ok(contents) = fs::read_to_string(&file) else {
            continue;
        };

        for (index, line) in contents.lines().enumerate() {
            // Ignore the doc comments that describe the rule itself.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }

            if line.contains("adk_") || line.contains("adk-") {
                violations.push(format!(
                    "{}:{}: {}",
                    file.strip_prefix(crate_root).unwrap_or(&file).display(),
                    index + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "ADK types leaked outside {ALLOWED_DIR}:\n{}",
        violations.join("\n")
    );
}
