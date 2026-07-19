use anyhow::{Context, Result, anyhow};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const LOG_ENV: &str = "ANIHUB_LOG";
const LOG_FILE_NAME: &str = "anihub-cli.log";
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

static LOGGER: OnceLock<Option<DiagnosticsLogger>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    const fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

struct DiagnosticsLogger {
    level: Level,
    file: Mutex<File>,
}

/// Enable structured diagnostics only when `ANIHUB_LOG` is set. Accepted
/// values: error, warn, info/1, debug, trace. Runtime logging failures are
/// intentionally non-fatal so diagnostics can never break playback.
pub fn init(data_dir: &Path) -> Result<Option<PathBuf>> {
    let configured = std::env::var(LOG_ENV).ok();
    let Some(level) = configured.as_deref().and_then(parse_level) else {
        if configured
            .as_deref()
            .is_some_and(|value| !is_disabled(value))
        {
            return Err(anyhow!(
                "invalid {LOG_ENV} value; use error, warn, info, debug, trace, 1 or 0"
            ));
        }
        let _ = LOGGER.set(None);
        return Ok(None);
    };

    let log_dir = data_dir.join("logs");
    fs::create_dir_all(&log_dir).with_context(|| {
        format!(
            "failed to create diagnostics directory {}",
            log_dir.display()
        )
    })?;
    let path = log_dir.join(LOG_FILE_NAME);
    rotate_if_needed(&path, MAX_LOG_BYTES)?;
    let file = open_private_append(&path)?;
    LOGGER
        .set(Some(DiagnosticsLogger {
            level,
            file: Mutex::new(file),
        }))
        .map_err(|_| anyhow!("diagnostics were already initialized"))?;
    Ok(Some(path))
}

fn parse_level(value: &str) -> Option<Level> {
    match value.trim().to_ascii_lowercase().as_str() {
        "error" => Some(Level::Error),
        "warn" | "warning" => Some(Level::Warn),
        "1" | "on" | "true" | "info" => Some(Level::Info),
        "debug" => Some(Level::Debug),
        "trace" => Some(Level::Trace),
        _ => None,
    }
}

fn is_disabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "off" | "false"
    )
}

fn open_private_append(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("failed to open diagnostics log {}", path.display()))
}

fn rotate_if_needed(path: &Path, max_bytes: u64) -> Result<()> {
    if fs::metadata(path).map_or(true, |metadata| metadata.len() < max_bytes) {
        return Ok(());
    }
    let backup = path.with_extension("log.1");
    if backup.exists() {
        fs::remove_file(&backup).with_context(|| {
            format!("failed to replace diagnostics backup {}", backup.display())
        })?;
    }
    fs::rename(path, &backup).with_context(|| {
        format!(
            "failed to rotate diagnostics log {} into {}",
            path.display(),
            backup.display()
        )
    })
}

fn write(level: Level, event: &'static str, fields: Value) {
    let Some(logger) = LOGGER.get().and_then(Option::as_ref) else {
        return;
    };
    if level > logger.level {
        return;
    }
    let record = json!({
        "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "level": level.label(),
        "event": event,
        "fields": fields,
    });
    let Ok(mut file) = logger.file.lock() else {
        return;
    };
    let _ = serde_json::to_writer(&mut *file, &record);
    let _ = file.write_all(b"\n");
}

pub fn error(event: &'static str, fields: Value) {
    write(Level::Error, event, fields);
}

pub fn warn(event: &'static str, fields: Value) {
    write(Level::Warn, event, fields);
}

pub fn info(event: &'static str, fields: Value) {
    write(Level::Info, event, fields);
}

pub fn debug(event: &'static str, fields: Value) {
    write(Level::Debug, event, fields);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_explicit_levels_and_disabled_values() {
        assert_eq!(parse_level("1"), Some(Level::Info));
        assert_eq!(parse_level(" DEBUG "), Some(Level::Debug));
        assert_eq!(parse_level("trace"), Some(Level::Trace));
        assert_eq!(parse_level("nope"), None);
        assert!(is_disabled("0"));
        assert!(is_disabled("off"));
    }

    #[test]
    fn rotates_a_full_log_once() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("anihub-diagnostics-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(LOG_FILE_NAME);
        fs::write(&path, b"full log").unwrap();

        rotate_if_needed(&path, 4).unwrap();

        assert!(!path.exists());
        assert_eq!(fs::read(path.with_extension("log.1")).unwrap(), b"full log");
        fs::remove_dir_all(directory).unwrap();
    }
}
