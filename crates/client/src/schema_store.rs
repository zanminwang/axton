//! Which file a client opens, and the schema each file was built for.
//!
//! The application names one `path`. The file actually in use is recorded in
//! the sidecar `<path>.current` (one line, a file name; absent means `path`).
//! An incompatible schema gets a fresh file `<path>.<n>` and the sidecar moves
//! to it once the new file is initialised; the old file is never deleted.
use crate::store::ClientStore;
use axton_core::{Result, Schema, canonical_json, invalid};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const SIDECAR_SUFFIX: &str = ".current";

/// The file the sidecar points at, or `path` itself.
pub fn current_file(path: &Path) -> PathBuf {
    let sidecar = sidecar_of(path);
    match std::fs::read_to_string(&sidecar) {
        Ok(name) => {
            let name = name.trim();
            if name.is_empty() {
                path.to_path_buf()
            } else {
                path.with_file_name(name)
            }
        }
        Err(_) => path.to_path_buf(),
    }
}

pub fn sidecar_of(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(SIDECAR_SUFFIX);
    path.with_file_name(name)
}

/// Point the sidecar at `file`, atomically (write beside, then rename).
pub fn set_current_file(path: &Path, file: &Path) -> Result<()> {
    let sidecar = sidecar_of(path);
    let temp = sidecar.with_extension("current.tmp");
    let name = file
        .file_name()
        .ok_or_else(|| invalid("database file has no name"))?
        .to_string_lossy()
        .into_owned();
    std::fs::write(&temp, format!("{name}\n")).map_err(|e| invalid(e.to_string()))?;
    std::fs::rename(&temp, &sidecar).map_err(|e| invalid(e.to_string()))?;
    Ok(())
}

/// The generation number of `file`: 0 for `path` itself, `n` for `<path>.<n>`.
pub fn file_number(path: &Path, file: &Path) -> u64 {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    file.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix(&format!("{base}.")))
        .and_then(|rest| rest.parse().ok())
        .unwrap_or(0)
}

/// `<path>.<n>` one above every existing numbered file, so numbers only grow
/// even after the application deletes an old generation.
pub fn next_free_file(path: &Path) -> PathBuf {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let highest = numbered_files(path)
        .iter()
        .map(|f| file_number(path, f))
        .max()
        .unwrap_or(0);
    path.with_file_name(format!("{base}.{}", highest + 1))
}

/// Every `<path>.<n>` file beside `path`, whatever its state.
pub fn numbered_files(path: &Path) -> Vec<PathBuf> {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(dir) = path.parent() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    }) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(&format!("{base}.")))
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        })
        .collect();
    files.sort();
    files
}

/// Remove a database file and SQLite's companions beside it.
pub fn remove_database_files(file: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = file.as_os_str().to_owned();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
}

/// The canonical descriptor text stored for `schema`.
pub fn descriptor_text(schema: &Schema) -> Result<String> {
    canonical_json(&serde_json::to_value(schema)?)
}

/// The schema this database was built for, if it recorded one.
pub fn read_descriptor<S: ClientStore>(store: &mut S) -> Result<Option<Schema>> {
    let rows = store.query("SELECT descriptor FROM axton_schema LIMIT 1", &[])?;
    match rows.rows.first().and_then(|r| r[0].as_str()) {
        Some(text) => Ok(Some(Schema::from_value(serde_json::from_str::<Value>(
            text,
        )?)?)),
        None => Ok(None),
    }
}

/// Record `schema` as the one this database is built for (replacing any).
pub fn write_descriptor<S: ClientStore>(store: &mut S, schema: &Schema) -> Result<()> {
    store.execute("DELETE FROM axton_schema", &[])?;
    store.execute(
        "INSERT INTO axton_schema (descriptor, created_at) VALUES (?, ?)",
        &[
            json!(descriptor_text(schema)?),
            json!(chrono::Utc::now().to_rfc3339()),
        ],
    )?;
    Ok(())
}
