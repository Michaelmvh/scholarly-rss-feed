use serde::de::DeserializeOwned;
use serde::Serialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const CACHE_DIR_ENV: &str = "GSRF_CACHE_DIR";

pub fn load<T: DeserializeOwned>(name: &str) -> Result<Option<T>, String> {
    let Some(path) = cache_path(name)? else {
        return Ok(None);
    };
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("Failed to parse snapshot {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "Failed to read snapshot {}: {error}",
            path.display()
        )),
    }
}

pub fn save<T: Serialize>(name: &str, value: &T) -> Result<(), String> {
    let Some(path) = cache_path(name)? else {
        return Ok(());
    };
    let parent = path
        .parent()
        .ok_or_else(|| format!("Snapshot path {} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Failed to create snapshot directory {}: {error}",
            parent.display()
        )
    })?;
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("Failed to serialize snapshot: {error}"))?;
    let temporary = temporary_path(&path);
    fs::write(&temporary, bytes).map_err(|error| {
        format!(
            "Failed to write temporary snapshot {}: {error}",
            temporary.display()
        )
    })?;
    fs::rename(&temporary, &path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!(
            "Failed to replace snapshot {} with {}: {error}",
            path.display(),
            temporary.display()
        )
    })
}

fn cache_path(name: &str) -> Result<Option<PathBuf>, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
    {
        return Err(format!("Invalid snapshot name \"{name}\"."));
    }
    Ok(env::var_os(CACHE_DIR_ENV).map(|directory| PathBuf::from(directory).join(name)))
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}
