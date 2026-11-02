//! Offline migration history must not change after acceptance against retained data.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct History {
    schema_version: u8,
    migrations: Vec<Migration>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Migration {
    file: String,
    sha256: String,
}

pub fn violations(root: &Path) -> Result<Vec<String>, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let directory = root.join("crates/persistence/migrations");
    let history: History = serde_json::from_slice(
        &std::fs::read(directory.join("frozen.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("Invalid frozen migration history: {error}"))?;
    if history.schema_version != 1 || history.migrations.is_empty() {
        return Err("Unsupported or empty frozen migration history".into());
    }
    let mut versions = BTreeSet::new();
    let mut findings = Vec::new();
    for migration in history.migrations {
        let Some((version, name)) = migration.file.split_once('_') else {
            return Err("Invalid frozen migration filename".into());
        };
        if version.len() != 4
            || !version.bytes().all(|byte| byte.is_ascii_digit())
            || version == "0000"
            || !name.ends_with(".sql")
            || name.len() <= 4
            || !name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.".contains(&byte)
            })
            || !versions.insert(version.to_owned())
            || migration.sha256.len() != 64
            || !migration
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("Invalid or duplicate frozen migration entry".into());
        }
        let path = directory.join(&migration.file);
        let Ok(resolved) = path.canonicalize() else {
            findings.push(format!("Frozen migration {} is missing", migration.file));
            continue;
        };
        if !resolved.starts_with(&root) {
            findings.push(format!(
                "Frozen migration {} escapes the repository",
                migration.file
            ));
            continue;
        }
        let bytes = std::fs::read(&resolved).map_err(|error| error.to_string())?;
        if checksum(&bytes) != migration.sha256 {
            findings.push(format!(
                "Frozen migration {} changed; add a new migration instead",
                migration.file
            ));
        }
    }
    Ok(findings)
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const SQL: &str = "SELECT 1;\n";
    const HASH: &str = "b4e0497804e46e0a0b0b8c31975b062152d551bac49c3c2e80932567b4085dcd";

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("crates/persistence/migrations");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("0001_initial.sql"), SQL).unwrap();
        history(
            root.path(),
            json!([{"file":"0001_initial.sql","sha256":HASH}]),
        );
        root
    }
    fn history(root: &Path, rows: serde_json::Value) {
        std::fs::write(
            root.join("crates/persistence/migrations/frozen.json"),
            serde_json::to_vec(&json!({"schemaVersion":1,"migrations":rows})).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn frozen_bytes_pass_and_new_migrations_remain_possible() {
        let root = fixture();
        std::fs::write(
            root.path()
                .join("crates/persistence/migrations/0002_additive.sql"),
            "SELECT 2;",
        )
        .unwrap();
        assert!(violations(root.path()).unwrap().is_empty());
    }
    #[test]
    fn edited_or_removed_history_fails() {
        let root = fixture();
        let path = root
            .path()
            .join("crates/persistence/migrations/0001_initial.sql");
        std::fs::write(&path, "SELECT 2;\n").unwrap();
        assert!(violations(root.path()).unwrap()[0].contains("changed"));
        std::fs::remove_file(path).unwrap();
        assert!(violations(root.path()).unwrap()[0].contains("missing"));
    }
    #[test]
    fn same_version_cannot_be_replaced_by_another_filename() {
        let root = fixture();
        history(
            root.path(),
            json!([{"file":"0001_initial.sql","sha256":HASH},{"file":"0001_replacement.sql","sha256":HASH}]),
        );
        assert!(violations(root.path()).is_err());
    }
    #[test]
    fn malformed_paths_and_hashes_are_not_opened() {
        let root = fixture();
        for file in [
            "../outside.sql",
            "0001_../outside.sql",
            "0000_invalid.sql",
            "0001_.sql",
        ] {
            history(root.path(), json!([{"file":file,"sha256":HASH}]));
            assert!(violations(root.path()).is_err());
        }
        history(
            root.path(),
            json!([{"file":"0001_initial.sql","sha256":"bad"}]),
        );
        assert!(violations(root.path()).is_err());
    }
    #[test]
    fn linked_domain_migration_is_checked_and_external_target_is_denied() {
        let root = fixture();
        let link = root
            .path()
            .join("crates/persistence/migrations/0001_initial.sql");
        let target = root.path().join("domain.sql");
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&target, SQL).unwrap();
        std::os::unix::fs::symlink("../../../domain.sql", &link).unwrap();
        assert!(violations(root.path()).unwrap().is_empty());
        std::fs::write(target, "SELECT 2;").unwrap();
        assert!(violations(root.path()).unwrap()[0].contains("changed"));
        std::fs::remove_file(&link).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert!(violations(root.path()).unwrap()[0].contains("escapes"));
    }
}
