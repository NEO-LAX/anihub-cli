#[cfg(windows)]
use anyhow::anyhow;
use anyhow::{Context, Result};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
#[cfg(windows)]
use std::io::ErrorKind;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".to_string());
    loop {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = path.with_file_name(format!(
            "{file_name}{suffix}{timestamp}-{}",
            TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
}

/// Write a complete sibling file, sync it, and only then atomically replace
/// the destination. A failed replacement never deletes the last valid file.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = unique_sibling_path(path, ".tmp-");
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("не вдалося створити {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("не вдалося записати {}", temporary.display()))?;
        file.flush()
            .with_context(|| format!("не вдалося завершити запис {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("не вдалося синхронізувати {}", temporary.display()))?;
        drop(file);

        replace_file(&temporary, path)?;
        sync_parent_directory(path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    fs::rename(temporary, destination).with_context(|| {
        format!(
            "не вдалося атомарно замінити {} файлом {}",
            destination.display(),
            temporary.display()
        )
    })
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    let destination_exists = fs::symlink_metadata(destination)
        .map(|_| true)
        .or_else(|error| {
            if error.kind() == ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(error)
            }
        })
        .with_context(|| format!("не вдалося перевірити {}", destination.display()))?;
    if !destination_exists {
        return fs::rename(temporary, destination)
            .with_context(|| format!("не вдалося встановити {}", destination.display()));
    }

    let displaced = unique_sibling_path(destination, ".previous-");
    fs::rename(destination, &displaced)
        .with_context(|| format!("не вдалося відкласти {}", destination.display()))?;
    match fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(displaced);
            Ok(())
        }
        Err(error) => match fs::rename(&displaced, destination) {
            Ok(()) => Err(anyhow!(
                "не вдалося замінити {}: {error}",
                destination.display()
            )),
            Err(restore_error) => Err(anyhow!(
                "не вдалося замінити {}: {error}; відновлення також не вдалося: {restore_error}",
                destination.display()
            )),
        },
    }
}

fn sync_parent_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        File::open(parent)
            .with_context(|| format!("не вдалося відкрити {}", parent.display()))?
            .sync_all()
            .with_context(|| format!("не вдалося синхронізувати {}", parent.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file_without_leaving_temporary_files() {
        let directory = std::env::temp_dir().join(format!(
            "anihub-atomic-write-{}-{}",
            std::process::id(),
            TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.json");
        fs::write(&path, b"old").unwrap();

        write(&path, b"new").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
