use anyhow::{Context, Result};
use chrono::Utc;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

const HISTORY_SCHEMA_VERSION: u32 = 1;
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct WatchProgress {
    pub anime_id: u32,
    pub anime_title: String,
    pub season: u32,
    pub episode: u32,
    #[serde(default)]
    pub studio_name: String, // Назва студії (озвучки)
    #[serde(default)]
    pub timestamp: f64, // час у секундах, на якому зупинився користувач
    #[serde(default)]
    pub duration: f64, // загальна тривалість епізоду, якщо відома
    #[serde(default)]
    pub watched: bool, // чи вважається серія переглянутою
    #[serde(default)]
    pub updated_at: i64, // Unix timestamp для сортування "Продовжити перегляд"
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct AppHistory {
    /// Ключ - це ID аніме. Значення - прогрес перегляду.
    pub progress: HashMap<String, WatchProgress>,
    pub bookmarks: Vec<u32>, // Список ID аніме, які збережені в закладки
}

/// One watched-state change for [`StorageManager::set_episodes_watched`].
///
/// A slice of these updates is applied while holding one storage lock and is
/// persisted with one atomic write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeWatchedUpdate {
    pub anime_id: u32,
    pub anime_title: String,
    pub season: u32,
    pub episode: u32,
    pub studio_name: String,
    pub watched: bool,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(
        "corrupt history file `{primary}` was preserved as `{preserved_as}`; no valid backup was available (primary error: {primary_error}; backup: {backup_error:?})"
    )]
    CorruptHistory {
        primary: PathBuf,
        preserved_as: PathBuf,
        backup: Option<PathBuf>,
        primary_error: String,
        backup_error: Option<String>,
    },

    #[error(
        "corrupt history file `{primary}` was preserved as `{preserved_as}`, and backup `{backup}` was also invalid (primary error: {primary_error}; backup error: {backup_error})"
    )]
    CorruptHistoryAndBackup {
        primary: PathBuf,
        preserved_as: PathBuf,
        backup: PathBuf,
        primary_error: String,
        backup_error: String,
    },

    #[error("failed to preserve corrupt history file `{primary}` as `{preserved_as}`: {source}")]
    PreserveCorruptHistory {
        primary: PathBuf,
        preserved_as: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("history backup `{backup}` is corrupt while the primary file is missing: {error}")]
    CorruptBackup { backup: PathBuf, error: String },

    #[error("failed to restore history backup `{backup}` to `{primary}`: {error}")]
    RestoreBackup {
        primary: PathBuf,
        backup: PathBuf,
        error: String,
    },

    #[error("timed out waiting for storage lock `{path}`")]
    LockTimeout { path: PathBuf },
}

#[derive(Debug, Error)]
enum ParseHistoryError {
    #[error("invalid history JSON or data: {message}")]
    Invalid { message: String },

    #[error("unsupported history schema version {version}")]
    UnsupportedSchemaVersion { version: u64 },
}

#[derive(Debug, Deserialize)]
struct RawAppHistory {
    progress: HashMap<String, Value>,
    bookmarks: Vec<u32>,
}

#[derive(Debug, Serialize)]
struct HistoryEnvelope {
    schema_version: u32,
    #[serde(flatten)]
    history: AppHistory,
}

#[derive(Debug)]
struct LoadedHistory {
    history: AppHistory,
    /// Exact bytes of the valid primary file, if one existed. Keeping these
    /// bytes lets save_history make the backup from the file that was read.
    primary_bytes: Option<Vec<u8>>,
}

pub struct StorageManager {
    history_file: PathBuf,
}

impl StorageManager {
    pub fn make_progress_key(
        anime_id: u32,
        season: u32,
        episode: u32,
        studio_name: &str,
    ) -> String {
        format!("{anime_id}:{season}:{episode}:{studio_name}")
    }

    pub fn new() -> Result<Self> {
        let proj_dirs = ProjectDirs::from("com", "shadowgarden", "anihub-cli")
            .context("Failed to determine project directories")?;

        let data_dir = proj_dirs.data_dir();

        // Створюємо директорію, якщо її не існує
        if !data_dir.exists() {
            fs::create_dir_all(data_dir).context("Failed to create data directory")?;
        }

        let history_file = data_dir.join("history.json");

        Ok(Self { history_file })
    }

    pub fn load_history(&self) -> Result<AppHistory> {
        let _lock = StorageLock::acquire(&self.lock_path())?;
        Ok(self.load_history_locked()?.history)
    }

    pub fn save_history(&self, history: &AppHistory) -> Result<()> {
        let content = serialize_history(history)?;
        let _lock = StorageLock::acquire(&self.lock_path())?;
        let loaded = self.load_history_locked()?;

        self.save_history_locked(history, loaded.primary_bytes.as_deref(), &content)
    }

    pub fn compute_watched(timestamp: f64, duration: f64) -> bool {
        if duration > 0.0 {
            (timestamp / duration >= 0.80) || (timestamp >= 1200.0)
        } else {
            timestamp >= 1200.0
        }
    }

    /// Apply a group of in-memory changes in one read-modify-write
    /// transaction. The callback runs while the cross-process lock is held.
    pub fn update_history_batch<F>(&self, mutation: F) -> Result<AppHistory>
    where
        F: FnOnce(&mut AppHistory),
    {
        let _lock = StorageLock::acquire(&self.lock_path())?;
        let loaded = self.load_history_locked()?;
        let mut history = loaded.history;

        mutation(&mut history);

        let content = serialize_history(&history)?;
        self.save_history_locked(&history, loaded.primary_bytes.as_deref(), &content)?;

        Ok(history)
    }

    /// Update several episodes with one lock acquisition and one atomic save.
    /// If the same key appears more than once, the last update in the slice
    /// wins deterministically.
    pub fn set_episodes_watched(&self, updates: &[EpisodeWatchedUpdate]) -> Result<AppHistory> {
        let now = Utc::now().timestamp();
        self.update_history_batch(|history| {
            for update in updates {
                Self::apply_episode_watched(history, update, now);
            }
        })
    }

    /// Оновлює прогрес серії та повертає нову AppHistory (щоб уникнути зайвого читання з диску).
    #[allow(clippy::too_many_arguments)]
    pub fn update_progress(
        &self,
        anime_id: u32,
        title: &str,
        season: u32,
        episode: u32,
        studio_name: &str,
        timestamp: f64,
        duration: f64,
    ) -> Result<AppHistory> {
        let progress = WatchProgress {
            anime_id,
            anime_title: title.to_string(),
            season,
            episode,
            studio_name: studio_name.to_string(),
            timestamp,
            duration,
            watched: Self::compute_watched(timestamp, duration),
            updated_at: Utc::now().timestamp(),
        };

        self.update_history_batch(|history| {
            history.progress.insert(
                Self::make_progress_key(anime_id, season, episode, studio_name),
                progress,
            );
        })
    }

    #[allow(dead_code)]
    pub fn delete_anime_progress(&self, anime_id: u32) -> Result<()> {
        self.update_history_batch(|history| {
            history
                .progress
                .retain(|_, progress| progress.anime_id != anime_id);
        })
        .map(|_| ())
    }

    pub fn delete_anime_progresses(&self, anime_ids: &[u32]) -> Result<()> {
        self.update_history_batch(|history| {
            history
                .progress
                .retain(|_, progress| !anime_ids.contains(&progress.anime_id));
        })
        .map(|_| ())
    }

    #[allow(dead_code)]
    pub fn reset_season_watched(&self, anime_id: u32, season: u32) -> Result<()> {
        let now = Utc::now().timestamp();
        self.update_history_batch(|history| {
            for progress in history.progress.values_mut() {
                if progress.anime_id == anime_id && progress.season == season {
                    progress.watched = false;
                    progress.updated_at = now;
                }
            }
        })
        .map(|_| ())
    }

    #[allow(dead_code)]
    pub fn reset_episode_progress(
        &self,
        anime_id: u32,
        season: u32,
        episode: u32,
        studio_name: &str,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        self.update_history_batch(|history| {
            if let Some(progress) = history.progress.values_mut().find(|progress| {
                progress.anime_id == anime_id
                    && progress.season == season
                    && progress.episode == episode
                    && progress.studio_name == studio_name
            }) {
                if progress.watched {
                    progress.watched = false;
                } else if progress.timestamp > 0.0 {
                    progress.timestamp = 0.0;
                }
                progress.updated_at = now;
            }
        })
        .map(|_| ())
    }

    pub fn set_episode_watched(
        &self,
        anime_id: u32,
        title: &str,
        season: u32,
        episode: u32,
        studio_name: &str,
        watched: bool,
    ) -> Result<()> {
        let update = EpisodeWatchedUpdate {
            anime_id,
            anime_title: title.to_string(),
            season,
            episode,
            studio_name: studio_name.to_string(),
            watched,
        };

        self.set_episodes_watched(std::slice::from_ref(&update))
            .map(|_| ())
    }

    #[allow(dead_code)]
    pub fn latest_progress(&self) -> Result<Option<WatchProgress>> {
        let history = self.load_history()?;
        Ok(history
            .progress
            .values()
            .max_by_key(|progress| progress.updated_at)
            .cloned())
    }

    #[allow(dead_code)]
    pub fn latest_progress_for_anime(&self, anime_id: u32) -> Result<Option<WatchProgress>> {
        let history = self.load_history()?;
        Ok(history
            .progress
            .values()
            .filter(|progress| progress.anime_id == anime_id)
            .max_by_key(|progress| progress.updated_at)
            .cloned())
    }

    #[allow(dead_code)]
    pub fn toggle_bookmark(&self, anime_id: u32) -> Result<bool> {
        let mut is_bookmarked = false;

        self.update_history_batch(|history| {
            if let Some(pos) = history.bookmarks.iter().position(|&x| x == anime_id) {
                history.bookmarks.remove(pos); // Якщо вже є - видаляємо (зняти закладку)
            } else {
                history.bookmarks.push(anime_id); // Якщо немає - додаємо
                is_bookmarked = true;
            }
        })?;

        Ok(is_bookmarked)
    }

    fn apply_episode_watched(history: &mut AppHistory, update: &EpisodeWatchedUpdate, now: i64) {
        let key = Self::make_progress_key(
            update.anime_id,
            update.season,
            update.episode,
            &update.studio_name,
        );

        match history.progress.get_mut(&key) {
            Some(progress) => {
                progress.anime_title = update.anime_title.clone();
                progress.watched = update.watched;
                if update.watched {
                    if progress.timestamp <= 0.0 {
                        progress.timestamp = progress.duration.max(1200.0);
                    }
                } else {
                    progress.timestamp = 0.0;
                }
                progress.updated_at = now;
            }
            None => {
                history.progress.insert(
                    key,
                    WatchProgress {
                        anime_id: update.anime_id,
                        anime_title: update.anime_title.clone(),
                        season: update.season,
                        episode: update.episode,
                        studio_name: update.studio_name.clone(),
                        timestamp: if update.watched { 1200.0 } else { 0.0 },
                        duration: 0.0,
                        watched: update.watched,
                        updated_at: now,
                    },
                );
            }
        }
    }

    fn lock_path(&self) -> PathBuf {
        append_path_suffix(&self.history_file, ".lock")
    }

    fn backup_path(&self) -> PathBuf {
        append_path_suffix(&self.history_file, ".bak")
    }

    fn load_history_locked(&self) -> Result<LoadedHistory> {
        let primary_bytes = read_optional_file(&self.history_file)?;

        match primary_bytes {
            Some(bytes) => match parse_history(&bytes) {
                Ok(history) => Ok(LoadedHistory {
                    history,
                    primary_bytes: Some(bytes),
                }),
                Err(primary_error) => self.recover_corrupt_primary(primary_error),
            },
            None => self.recover_missing_primary(),
        }
    }

    fn recover_corrupt_primary(&self, primary_error: ParseHistoryError) -> Result<LoadedHistory> {
        let preserved_as = self.preserve_corrupt_primary()?;
        let backup = self.backup_path();
        let primary_error = primary_error.to_string();

        let backup_bytes = match read_optional_file(&backup) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                return Err(StorageError::CorruptHistory {
                    primary: self.history_file.clone(),
                    preserved_as,
                    backup: None,
                    primary_error,
                    backup_error: None,
                }
                .into());
            }
            Err(error) => {
                return Err(StorageError::CorruptHistory {
                    primary: self.history_file.clone(),
                    preserved_as,
                    backup: Some(backup),
                    primary_error,
                    backup_error: Some(error.to_string()),
                }
                .into());
            }
        };

        match parse_history(&backup_bytes) {
            Ok(history) => {
                self.restore_backup(&backup, &backup_bytes)?;
                Ok(LoadedHistory {
                    history,
                    primary_bytes: Some(backup_bytes),
                })
            }
            Err(backup_error) => Err(StorageError::CorruptHistoryAndBackup {
                primary: self.history_file.clone(),
                preserved_as,
                backup,
                primary_error,
                backup_error: backup_error.to_string(),
            }
            .into()),
        }
    }

    fn recover_missing_primary(&self) -> Result<LoadedHistory> {
        let backup = self.backup_path();
        let backup_bytes = match read_optional_file(&backup)? {
            Some(bytes) => bytes,
            None => {
                return Ok(LoadedHistory {
                    history: AppHistory::default(),
                    primary_bytes: None,
                });
            }
        };

        let history =
            parse_history(&backup_bytes).map_err(|error| StorageError::CorruptBackup {
                backup: backup.clone(),
                error: error.to_string(),
            })?;

        self.restore_backup(&backup, &backup_bytes)?;
        Ok(LoadedHistory {
            history,
            primary_bytes: Some(backup_bytes),
        })
    }

    fn preserve_corrupt_primary(&self) -> Result<PathBuf> {
        let preserved_as = unique_sibling_path(&self.history_file, ".corrupt-");
        fs::rename(&self.history_file, &preserved_as).map_err(|source| {
            StorageError::PreserveCorruptHistory {
                primary: self.history_file.clone(),
                preserved_as: preserved_as.clone(),
                source,
            }
        })?;
        Ok(preserved_as)
    }

    fn restore_backup(&self, backup: &Path, bytes: &[u8]) -> Result<()> {
        atomic_write_file(&self.history_file, bytes).map_err(|error| {
            StorageError::RestoreBackup {
                primary: self.history_file.clone(),
                backup: backup.to_path_buf(),
                error: error.to_string(),
            }
        })?;
        Ok(())
    }

    fn save_history_locked(
        &self,
        _history: &AppHistory,
        previous_primary: Option<&[u8]>,
        content: &str,
    ) -> Result<()> {
        if let Some(previous_primary) = previous_primary {
            atomic_write_file(&self.backup_path(), previous_primary).with_context(|| {
                format!(
                    "failed to preserve the previous valid history as {}",
                    self.backup_path().display()
                )
            })?;
        }

        atomic_write_file(&self.history_file, content.as_bytes()).with_context(|| {
            format!(
                "failed to atomically save history to {}",
                self.history_file.display()
            )
        })?;

        Ok(())
    }
}

fn serialize_history(history: &AppHistory) -> Result<String> {
    serde_json::to_string_pretty(&HistoryEnvelope {
        schema_version: HISTORY_SCHEMA_VERSION,
        history: history.clone(),
    })
    .context("Failed to serialize history")
}

fn parse_history(bytes: &[u8]) -> std::result::Result<AppHistory, ParseHistoryError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| ParseHistoryError::Invalid {
            message: error.to_string(),
        })?;

    let history_value = match value {
        Value::Object(mut object) => match object.remove("schema_version") {
            None => Value::Object(object),
            Some(version) => {
                let version = version.as_u64().ok_or_else(|| ParseHistoryError::Invalid {
                    message: "schema_version must be a non-negative integer".to_string(),
                })?;

                if version != u64::from(HISTORY_SCHEMA_VERSION) {
                    return Err(ParseHistoryError::UnsupportedSchemaVersion { version });
                }

                // The canonical writer uses a flattened envelope. Accept a
                // nested `history` object as well so the version marker can
                // evolve without making recovery less tolerant.
                if let Some(history) = object.remove("history") {
                    history
                } else if let Some(history) = object.remove("data") {
                    history
                } else {
                    Value::Object(object)
                }
            }
        },
        _ => {
            return Err(ParseHistoryError::Invalid {
                message: "history root must be a JSON object".to_string(),
            });
        }
    };

    let raw: RawAppHistory =
        serde_json::from_value(history_value).map_err(|error| ParseHistoryError::Invalid {
            message: error.to_string(),
        })?;

    let mut migrated = BTreeMap::<String, (String, WatchProgress)>::new();
    for (source_key, raw_progress) in raw.progress {
        let watched_was_explicit = raw_progress
            .as_object()
            .is_some_and(|object| object.contains_key("watched"));

        let mut progress: WatchProgress =
            serde_json::from_value(raw_progress).map_err(|error| ParseHistoryError::Invalid {
                message: format!("progress `{source_key}`: {error}"),
            })?;

        // The old format did not have `watched`. Only those records are
        // derived; an explicit false is a user decision and must survive.
        if !watched_was_explicit {
            progress.watched =
                StorageManager::compute_watched(progress.timestamp, progress.duration);
        }

        let migrated_key = StorageManager::make_progress_key(
            progress.anime_id,
            progress.season,
            progress.episode,
            &progress.studio_name,
        );

        let replace =
            migrated
                .get(&migrated_key)
                .is_none_or(|(existing_source_key, existing_progress)| {
                    progress.updated_at > existing_progress.updated_at
                        || (progress.updated_at == existing_progress.updated_at
                            && source_key.as_str() < existing_source_key.as_str())
                });

        if replace {
            migrated.insert(migrated_key, (source_key, progress));
        }
    }

    Ok(AppHistory {
        progress: migrated
            .into_iter()
            .map(|(key, (_, progress))| (key, progress))
            .collect(),
        bookmarks: raw.bookmarks,
    })
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read history storage file {}", path.display())),
    }
}

fn serialize_timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn append_path_suffix(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "history.json".to_string());
    path.with_file_name(format!("{file_name}{suffix}"))
}

fn unique_sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "history.json".to_string());

    loop {
        let candidate = path.with_file_name(format!(
            "{file_name}{suffix}{}-{}",
            serialize_timestamp(),
            TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));

        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    unique_sibling_path(path, ".tmp-")
}

fn atomic_write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("failed to create temporary file {}", temporary.display()))?;

        file.write_all(bytes)
            .with_context(|| format!("failed to write temporary file {}", temporary.display()))?;
        file.flush()
            .with_context(|| format!("failed to flush temporary file {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync temporary file {}", temporary.display()))?;
        drop(file);

        replace_file(&temporary, path)?;
        sync_parent_directory(path)?;
        Ok(())
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
            "failed to atomically replace {} with {}",
            destination.display(),
            temporary.display()
        )
    })
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    // std::fs::rename does not replace an existing file on Windows. Move the
    // old destination aside only after its replacement is fully synced. If
    // the second rename fails, restore the old destination before returning.
    let destination_exists = fs::symlink_metadata(destination)
        .map(|_| true)
        .or_else(|error| {
            if error.kind() == ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(error)
            }
        })
        .with_context(|| format!("failed to inspect {}", destination.display()))?;

    if !destination_exists {
        return fs::rename(temporary, destination).with_context(|| {
            format!(
                "failed to move {} into {}",
                temporary.display(),
                destination.display()
            )
        });
    }

    let displaced = unique_sibling_path(destination, ".previous-");
    fs::rename(destination, &displaced).with_context(|| {
        format!(
            "failed to move the previous file {} aside",
            destination.display()
        )
    })?;

    match fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(displaced);
            Ok(())
        }
        Err(error) => {
            let restore_error = fs::rename(&displaced, destination).err();
            if let Some(restore_error) = restore_error {
                Err(anyhow::anyhow!(
                    "failed to replace {}: {error}; restoring the previous file also failed: {restore_error}",
                    destination.display()
                ))
            } else {
                Err(anyhow::anyhow!(
                    "failed to replace {}: {error}",
                    destination.display()
                ))
            }
        }
    }
}

fn sync_parent_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        File::open(parent)
            .with_context(|| format!("failed to open directory {} for syncing", parent.display()))?
            .sync_all()
            .with_context(|| format!("failed to sync directory {}", parent.display()))?;
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
}

struct StorageLock {
    path: PathBuf,
    _file: File,
}

impl StorageLock {
    fn acquire(path: &Path) -> Result<Self> {
        let started = std::time::Instant::now();

        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    let owner = format!(
                        "pid={}\ncreated_at={}\n",
                        process::id(),
                        serialize_timestamp()
                    );

                    if let Err(error) = file
                        .write_all(owner.as_bytes())
                        .and_then(|_| file.flush())
                        .and_then(|_| file.sync_all())
                    {
                        let _ = fs::remove_file(path);
                        return Err(error).with_context(|| {
                            format!("failed to initialize storage lock {}", path.display())
                        });
                    }

                    return Ok(Self {
                        path: path.to_path_buf(),
                        _file: file,
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if started.elapsed() >= LOCK_WAIT_TIMEOUT {
                        return Err(StorageError::LockTimeout {
                            path: path.to_path_buf(),
                        }
                        .into());
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to acquire storage lock {}", path.display())
                    });
                }
            }
        }
    }
}

impl Drop for StorageLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "anihub-cli-storage-test-{}-{}",
                process::id(),
                serialize_timestamp()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self { path }
        }

        fn manager(&self) -> StorageManager {
            StorageManager {
                history_file: self.path.join("history.json"),
            }
        }

        fn history_path(&self) -> PathBuf {
            self.path.join("history.json")
        }

        fn backup_path(&self) -> PathBuf {
            self.path.join("history.json.bak")
        }

        fn corrupt_paths(&self) -> Vec<PathBuf> {
            fs::read_dir(&self.path)
                .expect("read test directory")
                .filter_map(|entry| {
                    let path = entry.ok()?.path();
                    let name = path.file_name()?.to_string_lossy();
                    name.starts_with("history.json.corrupt-").then_some(path)
                })
                .collect()
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn progress(
        anime_id: u32,
        title: &str,
        season: u32,
        episode: u32,
        studio_name: &str,
        watched: bool,
        updated_at: i64,
    ) -> WatchProgress {
        WatchProgress {
            anime_id,
            anime_title: title.to_string(),
            season,
            episode,
            studio_name: studio_name.to_string(),
            timestamp: if watched { 1200.0 } else { 10.0 },
            duration: 1500.0,
            watched,
            updated_at,
        }
    }

    #[test]
    fn reads_legacy_bare_history_and_migrates_keys() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let legacy = json!({
            "progress": {
                "7": {
                    "anime_id": 7,
                    "anime_title": "Legacy",
                    "season": 2,
                    "episode": 3,
                    "studio_name": "Dub",
                    "timestamp": 1000.0,
                    "duration": 2000.0,
                    "updated_at": 10
                }
            },
            "bookmarks": [7]
        });
        fs::write(
            directory.history_path(),
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy history"),
        )
        .expect("write legacy history");

        let loaded = manager.load_history().expect("load legacy history");
        let key = StorageManager::make_progress_key(7, 2, 3, "Dub");
        assert_eq!(loaded.bookmarks, vec![7]);
        assert!(!loaded.progress.get(&key).expect("migrated record").watched);

        manager
            .save_history(&loaded)
            .expect("save migrated history");
        let saved: Value = serde_json::from_slice(
            &fs::read(directory.history_path()).expect("read saved history"),
        )
        .expect("parse saved history");
        assert_eq!(saved["schema_version"], json!(1));
    }

    #[test]
    fn migration_preserves_explicit_watched_false() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let legacy = json!({
            "progress": {
                "explicit-false": {
                    "anime_id": 8,
                    "anime_title": "Explicit false",
                    "season": 1,
                    "episode": 1,
                    "timestamp": 1500.0,
                    "duration": 1600.0,
                    "watched": false,
                    "updated_at": 11
                },
                "missing-watched": {
                    "anime_id": 8,
                    "anime_title": "Derived",
                    "season": 1,
                    "episode": 2,
                    "timestamp": 1500.0,
                    "duration": 1600.0,
                    "updated_at": 12
                }
            },
            "bookmarks": []
        });
        fs::write(
            directory.history_path(),
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy history"),
        )
        .expect("write legacy history");

        let loaded = manager.load_history().expect("load history");
        let explicit_key = StorageManager::make_progress_key(8, 1, 1, "");
        let derived_key = StorageManager::make_progress_key(8, 1, 2, "");
        assert!(!loaded.progress[&explicit_key].watched);
        assert!(loaded.progress[&derived_key].watched);
    }

    #[test]
    fn corrupt_primary_is_preserved_and_valid_backup_is_restored() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let original = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(1, 1, 1, "Dub"),
                progress(1, "Original", 1, 1, "Dub", false, 1),
            )]),
            bookmarks: vec![1],
        };
        let newer = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(2, 1, 1, "Dub"),
                progress(2, "Newer", 1, 1, "Dub", true, 2),
            )]),
            bookmarks: vec![2],
        };
        manager.save_history(&original).expect("save original");
        manager.save_history(&newer).expect("save newer");
        fs::write(directory.history_path(), b"{ not valid json").expect("corrupt primary");

        let restored = manager.load_history().expect("restore valid backup");
        assert_eq!(restored, original);
        assert_eq!(
            fs::read(directory.backup_path()).expect("read backup"),
            fs::read(directory.history_path()).expect("read restored primary")
        );
        assert_eq!(directory.corrupt_paths().len(), 1);
    }

    #[test]
    fn corrupt_primary_and_backup_return_error_without_defaulting() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let primary_bytes = b"{ primary is corrupt";
        let backup_bytes = b"{ backup is corrupt";
        fs::write(directory.history_path(), primary_bytes).expect("write primary");
        fs::write(directory.backup_path(), backup_bytes).expect("write backup");

        let error = manager.load_history().expect_err("corrupt files must fail");
        assert!(error.to_string().contains("corrupt history file"));
        assert!(error.downcast_ref::<StorageError>().is_some());
        assert!(!directory.history_path().exists());
        assert_eq!(
            fs::read(directory.backup_path()).expect("read corrupt backup"),
            backup_bytes
        );
        assert_eq!(directory.corrupt_paths().len(), 1);
    }

    #[test]
    fn saves_versioned_envelope_and_keeps_previous_valid_primary_as_backup() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let first = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(3, 1, 1, "Dub"),
                progress(3, "First", 1, 1, "Dub", false, 3),
            )]),
            bookmarks: vec![3],
        };
        let second = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(4, 1, 1, "Dub"),
                progress(4, "Second", 1, 1, "Dub", true, 4),
            )]),
            bookmarks: vec![4],
        };

        manager.save_history(&first).expect("save first");
        let first_on_disk = fs::read(directory.history_path()).expect("read first");
        let first_json: Value = serde_json::from_slice(&first_on_disk).expect("parse first");
        assert_eq!(first_json["schema_version"], json!(1));

        manager.save_history(&second).expect("save second");
        assert_eq!(
            fs::read(directory.backup_path()).expect("read backup"),
            first_on_disk
        );
        assert_eq!(manager.load_history().expect("load second"), second);

        let temporary_files = fs::read_dir(&directory.path)
            .expect("read test directory")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".tmp-")
            })
            .collect::<Vec<_>>();
        assert!(
            temporary_files.is_empty(),
            "temporary files remain: {temporary_files:?}"
        );
    }

    #[test]
    fn duplicate_migration_is_deterministic_by_timestamp_then_source_key() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let legacy = json!({
            "progress": {
                "z-source": {
                    "anime_id": 9,
                    "anime_title": "Z source",
                    "season": 1,
                    "episode": 1,
                    "studio_name": "Dub",
                    "timestamp": 20.0,
                    "duration": 100.0,
                    "watched": false,
                    "updated_at": 20
                },
                "a-source": {
                    "anime_id": 9,
                    "anime_title": "A source",
                    "season": 1,
                    "episode": 1,
                    "studio_name": "Dub",
                    "timestamp": 30.0,
                    "duration": 100.0,
                    "watched": true,
                    "updated_at": 20
                },
                "newer-source": {
                    "anime_id": 9,
                    "anime_title": "Newer source",
                    "season": 1,
                    "episode": 1,
                    "studio_name": "Dub",
                    "timestamp": 40.0,
                    "duration": 100.0,
                    "watched": false,
                    "updated_at": 21
                }
            },
            "bookmarks": []
        });
        fs::write(
            directory.history_path(),
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy history"),
        )
        .expect("write legacy history");

        let loaded = manager.load_history().expect("load migrated history");
        let key = StorageManager::make_progress_key(9, 1, 1, "Dub");
        assert_eq!(loaded.progress[&key].anime_title, "Newer source");

        let tie_only = json!({
            "progress": {
                "z-source": {
                    "anime_id": 10,
                    "anime_title": "Z source",
                    "season": 1,
                    "episode": 1,
                    "studio_name": "Dub",
                    "updated_at": 20
                },
                "a-source": {
                    "anime_id": 10,
                    "anime_title": "A source",
                    "season": 1,
                    "episode": 1,
                    "studio_name": "Dub",
                    "updated_at": 20
                }
            },
            "bookmarks": []
        });
        fs::write(
            directory.history_path(),
            serde_json::to_vec_pretty(&tie_only).expect("serialize tie history"),
        )
        .expect("write tie history");

        let tied = manager.load_history().expect("load tied history");
        let tie_key = StorageManager::make_progress_key(10, 1, 1, "Dub");
        assert_eq!(tied.progress[&tie_key].anime_title, "A source");
    }

    #[test]
    fn batch_episode_updates_share_one_transaction() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let initial = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(11, 1, 1, "Dub"),
                progress(11, "Initial", 1, 1, "Dub", false, 1),
            )]),
            bookmarks: vec![],
        };
        manager.save_history(&initial).expect("save initial");

        let updates = vec![
            EpisodeWatchedUpdate {
                anime_id: 11,
                anime_title: "Initial".to_string(),
                season: 1,
                episode: 1,
                studio_name: "Dub".to_string(),
                watched: true,
            },
            EpisodeWatchedUpdate {
                anime_id: 11,
                anime_title: "Initial".to_string(),
                season: 1,
                episode: 2,
                studio_name: "Dub".to_string(),
                watched: true,
            },
        ];

        let result = manager
            .set_episodes_watched(&updates)
            .expect("apply batch updates");
        assert_eq!(result.progress.len(), 2);
        assert!(result.progress.values().all(|progress| progress.watched));

        // A single transaction makes the backup the complete pre-batch
        // history. N individual saves would leave an intermediate state here.
        let backup_bytes = fs::read(directory.backup_path()).expect("read batch backup");
        let backup = parse_history(&backup_bytes).expect("parse batch backup");
        assert_eq!(backup, initial);
    }
}
