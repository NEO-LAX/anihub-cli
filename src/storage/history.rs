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

const HISTORY_SCHEMA_VERSION: u32 = 2;
const HISTORY_FILE_NAME: &str = "history.json";
const LEGACY_VERSIONED_HISTORY_FILE_NAME: &str = "history-v2.json";
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AnimeStatus {
    NotAdded,
    Planned,
    Watching,
    Completed,
    OnHold,
    Dropped,
}

impl AnimeStatus {
    pub const ALL: [Self; 6] = [
        Self::NotAdded,
        Self::Planned,
        Self::Watching,
        Self::Completed,
        Self::OnHold,
        Self::Dropped,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::NotAdded => "Не додано",
            Self::Planned => "У планах",
            Self::Watching => "Дивлюся",
            Self::Completed => "Переглянуто",
            Self::OnHold => "Відкладено",
            Self::Dropped => "Кинуто",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AnimeLibraryRecord {
    pub title: String,
    pub status: AnimeStatus,
    pub updated_at: i64,
    /// Release metadata lets the library render seasons, films and extras even
    /// before the user has started an episode from that release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<LibraryReleaseMetadata>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LibraryReleaseKind {
    Season,
    Movie,
    Special,
    Extra,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LibraryReleaseMetadata {
    pub title: String,
    pub kind: LibraryReleaseKind,
    pub season: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episodes_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airing_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_airing_episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_airing_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimeStatusUpdate {
    pub anime_id: u32,
    pub title: String,
    pub status: AnimeStatus,
    pub release: Option<LibraryReleaseMetadata>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct WatchProgress {
    pub anime_id: u32,
    pub anime_title: String,
    pub season: u32,
    pub episode: u32,
    pub studio_name: String, // Назва студії (озвучки)
    pub timestamp: f64,      // час у секундах, на якому зупинився користувач
    pub duration: f64,       // загальна тривалість епізоду, якщо відома
    pub watched: bool,       // чи вважається серія переглянутою
    pub updated_at: i64,     // Unix timestamp для сортування "Продовжити перегляд"
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppHistory {
    /// Ключ - це ID аніме. Значення - прогрес перегляду.
    pub progress: HashMap<String, WatchProgress>,
    pub library: HashMap<u32, AnimeLibraryRecord>,
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryEnvelope {
    schema_version: u32,
    progress: HashMap<String, WatchProgress>,
    library: HashMap<u32, AnimeLibraryRecord>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyHistory {
    progress: HashMap<String, Value>,
    bookmarks: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct LegacyWatchProgress {
    anime_id: u32,
    #[serde(default)]
    anime_title: String,
    #[serde(default)]
    season: u32,
    #[serde(default)]
    episode: u32,
    #[serde(default)]
    studio_name: String,
    #[serde(default)]
    timestamp: f64,
    #[serde(default)]
    duration: f64,
    watched: Option<bool>,
    #[serde(default)]
    updated_at: i64,
}

#[derive(Debug)]
struct ParsedHistory {
    history: AppHistory,
    migrated: bool,
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

        let history_file = data_dir.join(HISTORY_FILE_NAME);

        Ok(Self { history_file })
    }

    pub fn history_path(&self) -> &Path {
        &self.history_file
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

    pub fn compute_watched(timestamp: f64, duration: f64, threshold_percent: Option<u8>) -> bool {
        let Some(threshold) = threshold_percent else {
            return false;
        };
        duration > 0.0
            && timestamp.is_finite()
            && duration.is_finite()
            && timestamp / duration >= f64::from(threshold.clamp(1, 100)) / 100.0
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

    pub fn set_anime_status(
        &self,
        anime_ids: &[u32],
        title: &str,
        status: AnimeStatus,
    ) -> Result<AppHistory> {
        let now = Utc::now().timestamp();
        self.update_history_batch(|history| {
            for &anime_id in anime_ids {
                let release = history
                    .library
                    .get(&anime_id)
                    .and_then(|record| record.release.clone());
                history.library.insert(
                    anime_id,
                    AnimeLibraryRecord {
                        title: title.to_string(),
                        status,
                        updated_at: now,
                        release,
                    },
                );
            }
        })
    }

    /// Persist per-release statuses together with enough metadata to build the
    /// library without requiring episode progress or another catalog lookup.
    pub fn set_anime_statuses(&self, updates: &[AnimeStatusUpdate]) -> Result<AppHistory> {
        let now = Utc::now().timestamp();
        self.update_history_batch(|history| {
            for update in updates {
                let release = update.release.clone().or_else(|| {
                    history
                        .library
                        .get(&update.anime_id)
                        .and_then(|record| record.release.clone())
                });
                history.library.insert(
                    update.anime_id,
                    AnimeLibraryRecord {
                        title: update.title.clone(),
                        status: update.status,
                        updated_at: now,
                        release,
                    },
                );
            }
        })
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

    /// Atomically synchronize a release status and all of its watched flags.
    pub fn set_release_watched(
        &self,
        status_update: &AnimeStatusUpdate,
        episode_updates: &[EpisodeWatchedUpdate],
    ) -> Result<AppHistory> {
        self.set_releases_watched(std::slice::from_ref(status_update), episode_updates)
    }

    /// Atomically synchronize several release statuses and their watched flags.
    pub fn set_releases_watched(
        &self,
        status_updates: &[AnimeStatusUpdate],
        episode_updates: &[EpisodeWatchedUpdate],
    ) -> Result<AppHistory> {
        let now = Utc::now().timestamp();
        self.update_history_batch(|history| {
            for update in episode_updates {
                Self::apply_episode_watched(history, update, now);
            }
            for status_update in status_updates {
                let release = status_update.release.clone().or_else(|| {
                    history
                        .library
                        .get(&status_update.anime_id)
                        .and_then(|record| record.release.clone())
                });
                history.library.insert(
                    status_update.anime_id,
                    AnimeLibraryRecord {
                        title: status_update.title.clone(),
                        status: status_update.status,
                        updated_at: now,
                        release,
                    },
                );
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
        watched_threshold_percent: Option<u8>,
    ) -> Result<AppHistory> {
        let progress = WatchProgress {
            anime_id,
            anime_title: title.to_string(),
            season,
            episode,
            studio_name: studio_name.to_string(),
            timestamp,
            duration,
            watched: Self::compute_watched(timestamp, duration, watched_threshold_percent),
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

    pub fn delete_library_entries(&self, anime_ids: &[u32]) -> Result<AppHistory> {
        self.update_history_batch(|history| {
            history
                .progress
                .retain(|_, progress| !anime_ids.contains(&progress.anime_id));
            history
                .library
                .retain(|anime_id, _| !anime_ids.contains(anime_id));
        })
    }

    pub fn clear_library(&self) -> Result<AppHistory> {
        self.update_history_batch(|history| {
            history.progress.clear();
            history.library.clear();
        })
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

    /// Clear only the resume timestamp. The explicit watched flag is kept.
    pub fn clear_episode_timestamp(
        &self,
        anime_id: u32,
        season: u32,
        episode: u32,
        studio_name: &str,
    ) -> Result<AppHistory> {
        let now = Utc::now().timestamp();
        self.update_history_batch(|history| {
            if let Some(progress) = history.progress.values_mut().find(|progress| {
                progress.anime_id == anime_id
                    && progress.season == season
                    && progress.episode == episode
                    && progress.studio_name == studio_name
            }) {
                progress.timestamp = 0.0;
                progress.updated_at = now;
            }
        })
    }

    pub fn set_episode_watched(
        &self,
        anime_id: u32,
        title: &str,
        season: u32,
        episode: u32,
        studio_name: &str,
        watched: bool,
    ) -> Result<AppHistory> {
        let update = EpisodeWatchedUpdate {
            anime_id,
            anime_title: title.to_string(),
            season,
            episode,
            studio_name: studio_name.to_string(),
            watched,
        };

        self.set_episodes_watched(std::slice::from_ref(&update))
    }

    /// Toggle one logical episode across all known dubbings. The selected
    /// studio is inserted when a browser-only MoonAnime episode has no prior
    /// local progress row.
    pub fn set_episode_watched_across_dubbings(
        &self,
        anime_id: u32,
        title: &str,
        season: u32,
        episode: u32,
        studio_name: &str,
        watched: bool,
    ) -> Result<AppHistory> {
        let now = Utc::now().timestamp();
        let update = EpisodeWatchedUpdate {
            anime_id,
            anime_title: title.to_string(),
            season,
            episode,
            studio_name: studio_name.to_string(),
            watched,
        };
        self.update_history_batch(|history| {
            for progress in history.progress.values_mut().filter(|progress| {
                progress.anime_id == anime_id
                    && progress.season == season
                    && progress.episode == episode
            }) {
                progress.watched = watched;
                progress.updated_at = now;
            }
            Self::apply_episode_watched(history, &update, now);
        })
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
        self.import_versioned_history_if_needed()?;
        let primary_bytes = read_optional_file(&self.history_file)?;

        match primary_bytes {
            Some(bytes) => match parse_history_with_migration(&bytes) {
                Ok(parsed) if parsed.migrated => {
                    let content = serialize_history(&parsed.history)?;
                    atomic_write_file(&self.backup_path(), &bytes).with_context(|| {
                        format!(
                            "failed to preserve legacy history as {}",
                            self.backup_path().display()
                        )
                    })?;
                    atomic_write_file(&self.history_file, content.as_bytes()).with_context(
                        || {
                            format!(
                                "failed to migrate history to {}",
                                self.history_file.display()
                            )
                        },
                    )?;
                    Ok(LoadedHistory {
                        history: parsed.history,
                        primary_bytes: Some(content.into_bytes()),
                    })
                }
                Ok(parsed) => Ok(LoadedHistory {
                    history: parsed.history,
                    primary_bytes: Some(bytes),
                }),
                Err(primary_error) => self.recover_corrupt_primary(primary_error),
            },
            None => self.recover_missing_primary(),
        }
    }

    fn import_versioned_history_if_needed(&self) -> Result<()> {
        if self.history_file.exists() || self.backup_path().exists() {
            return Ok(());
        }

        let data_dir = self.history_file.parent().unwrap_or_else(|| Path::new("."));
        let legacy_primary = data_dir.join(LEGACY_VERSIONED_HISTORY_FILE_NAME);
        let legacy_backup = append_path_suffix(&legacy_primary, ".bak");
        let mut first_error = None;

        for candidate in [&legacy_primary, &legacy_backup] {
            let Some(bytes) = read_optional_file(candidate)? else {
                continue;
            };
            match parse_history_with_migration(&bytes) {
                Ok(parsed) => {
                    let content = serialize_history(&parsed.history)?;
                    atomic_write_file(&self.history_file, content.as_bytes()).with_context(
                        || {
                            format!(
                                "failed to import {} into {}",
                                candidate.display(),
                                self.history_file.display()
                            )
                        },
                    )?;
                    return Ok(());
                }
                Err(error) => {
                    first_error.get_or_insert_with(|| (candidate.to_path_buf(), error));
                }
            }
        }

        if let Some((path, error)) = first_error {
            anyhow::bail!(
                "legacy history {} could not be migrated: {error}",
                path.display()
            );
        }

        Ok(())
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

        match parse_history_with_migration(&backup_bytes) {
            Ok(parsed) => {
                let restored_bytes = if parsed.migrated {
                    serialize_history(&parsed.history)?.into_bytes()
                } else {
                    backup_bytes.clone()
                };
                self.restore_backup(&backup, &restored_bytes)?;
                Ok(LoadedHistory {
                    history: parsed.history,
                    primary_bytes: Some(restored_bytes),
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

        let parsed = parse_history_with_migration(&backup_bytes).map_err(|error| {
            StorageError::CorruptBackup {
                backup: backup.clone(),
                error: error.to_string(),
            }
        })?;
        let restored_bytes = if parsed.migrated {
            serialize_history(&parsed.history)?.into_bytes()
        } else {
            backup_bytes
        };

        self.restore_backup(&backup, &restored_bytes)?;
        Ok(LoadedHistory {
            history: parsed.history,
            primary_bytes: Some(restored_bytes),
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
        progress: history.progress.clone(),
        library: history.library.clone(),
    })
    .context("Failed to serialize history")
}

#[cfg(test)]
fn parse_history(bytes: &[u8]) -> std::result::Result<AppHistory, ParseHistoryError> {
    parse_history_with_migration(bytes).map(|parsed| parsed.history)
}

fn parse_history_with_migration(
    bytes: &[u8],
) -> std::result::Result<ParsedHistory, ParseHistoryError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| ParseHistoryError::Invalid {
            message: error.to_string(),
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| ParseHistoryError::Invalid {
            message: "history root must be a JSON object".to_string(),
        })?;
    let schema_version = object.get("schema_version").map(|version| {
        version.as_u64().ok_or_else(|| ParseHistoryError::Invalid {
            message: "schema_version must be a non-negative integer".to_string(),
        })
    });

    match schema_version.transpose()? {
        Some(version) if version == u64::from(HISTORY_SCHEMA_VERSION) => {
            parse_current_history(value)
        }
        Some(1) | None => migrate_legacy_history(value),
        Some(version) => Err(ParseHistoryError::UnsupportedSchemaVersion { version }),
    }
}

fn parse_current_history(value: Value) -> std::result::Result<ParsedHistory, ParseHistoryError> {
    let envelope: HistoryEnvelope =
        serde_json::from_value(value).map_err(|error| ParseHistoryError::Invalid {
            message: error.to_string(),
        })?;
    for (key, progress) in &envelope.progress {
        let expected = StorageManager::make_progress_key(
            progress.anime_id,
            progress.season,
            progress.episode,
            &progress.studio_name,
        );
        if *key != expected {
            return Err(ParseHistoryError::Invalid {
                message: format!("non-canonical progress key `{key}`; expected `{expected}`"),
            });
        }
    }
    Ok(ParsedHistory {
        history: AppHistory {
            progress: envelope.progress,
            library: envelope.library,
        },
        migrated: false,
    })
}

fn migrate_legacy_history(
    mut value: Value,
) -> std::result::Result<ParsedHistory, ParseHistoryError> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| ParseHistoryError::Invalid {
            message: "history root must be a JSON object".to_string(),
        })?;
    object.remove("schema_version");
    value = object
        .remove("history")
        .or_else(|| object.remove("data"))
        .unwrap_or_else(|| Value::Object(object.clone()));

    let legacy: LegacyHistory =
        serde_json::from_value(value).map_err(|error| ParseHistoryError::Invalid {
            message: error.to_string(),
        })?;
    let mut migrated = BTreeMap::<String, (String, WatchProgress)>::new();

    for (source_key, raw_progress) in legacy.progress {
        let progress: LegacyWatchProgress =
            serde_json::from_value(raw_progress).map_err(|error| ParseHistoryError::Invalid {
                message: format!("progress `{source_key}`: {error}"),
            })?;
        let watched = progress
            .watched
            .unwrap_or_else(|| legacy_compute_watched(progress.timestamp, progress.duration));
        let progress = WatchProgress {
            anime_id: progress.anime_id,
            anime_title: progress.anime_title,
            season: progress.season,
            episode: progress.episode,
            studio_name: progress.studio_name,
            timestamp: progress.timestamp,
            duration: progress.duration,
            watched,
            updated_at: progress.updated_at,
        };
        let canonical_key = StorageManager::make_progress_key(
            progress.anime_id,
            progress.season,
            progress.episode,
            &progress.studio_name,
        );
        let replace =
            migrated
                .get(&canonical_key)
                .is_none_or(|(existing_source_key, existing_progress)| {
                    progress.updated_at > existing_progress.updated_at
                        || (progress.updated_at == existing_progress.updated_at
                            && source_key.as_str() < existing_source_key.as_str())
                });
        if replace {
            migrated.insert(canonical_key, (source_key, progress));
        }
    }

    let progress = migrated
        .into_iter()
        .map(|(key, (_, progress))| (key, progress))
        .collect::<HashMap<_, _>>();
    let mut library = HashMap::<u32, AnimeLibraryRecord>::new();
    for item in progress.values() {
        let candidate = AnimeLibraryRecord {
            title: if item.anime_title.trim().is_empty() {
                format!("Аніме #{}", item.anime_id)
            } else {
                item.anime_title.clone()
            },
            status: AnimeStatus::Watching,
            updated_at: item.updated_at,
            release: None,
        };
        match library.get_mut(&item.anime_id) {
            Some(existing) if existing.updated_at < candidate.updated_at => *existing = candidate,
            None => {
                library.insert(item.anime_id, candidate);
            }
            _ => {}
        }
    }
    for anime_id in legacy.bookmarks {
        library
            .entry(anime_id)
            .or_insert_with(|| AnimeLibraryRecord {
                title: format!("Аніме #{anime_id}"),
                status: AnimeStatus::Planned,
                updated_at: 0,
                release: None,
            });
    }

    Ok(ParsedHistory {
        history: AppHistory { progress, library },
        migrated: true,
    })
}

fn legacy_compute_watched(timestamp: f64, duration: f64) -> bool {
    if duration > 0.0 {
        timestamp / duration >= 0.80 || timestamp >= 1200.0
    } else {
        timestamp >= 1200.0
    }
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
        .unwrap_or_else(|| HISTORY_FILE_NAME.to_string());
    path.with_file_name(format!("{file_name}{suffix}"))
}

fn unique_sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| HISTORY_FILE_NAME.to_string());

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
    use serde_json::{Value, json};

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
    fn migrates_previous_history_schema_and_bookmarks() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let legacy = serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "progress": {
                "42": {
                    "anime_id": 42,
                    "anime_title": "Legacy title",
                    "season": 1,
                    "episode": 3,
                    "timestamp": 1300.0,
                    "updated_at": 7
                }
            },
            "bookmarks": [42, 99]
        }))
        .expect("serialize old schema");
        fs::write(directory.history_path(), &legacy).expect("write legacy history");

        let migrated = manager.load_history().expect("migrate old schema");
        let key = StorageManager::make_progress_key(42, 1, 3, "");
        assert!(migrated.progress[&key].watched);
        assert_eq!(migrated.library[&42].status, AnimeStatus::Watching);
        assert_eq!(migrated.library[&99].status, AnimeStatus::Planned);
        assert_eq!(fs::read(directory.backup_path()).unwrap(), legacy);
        let canonical: Value =
            serde_json::from_slice(&fs::read(directory.history_path()).unwrap()).unwrap();
        assert_eq!(canonical["schema_version"], json!(2));
        assert!(canonical.get("bookmarks").is_none());
    }

    #[test]
    fn imports_versioned_v2_filename_without_deleting_it() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let expected = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(7, 2, 4, "Dub"),
                progress(7, "Versioned", 2, 4, "Dub", false, 12),
            )]),
            library: HashMap::new(),
        };
        let legacy_path = directory.path.join(LEGACY_VERSIONED_HISTORY_FILE_NAME);
        fs::write(&legacy_path, serialize_history(&expected).unwrap()).unwrap();

        assert_eq!(manager.load_history().unwrap(), expected);
        assert!(directory.history_path().exists());
        assert!(
            legacy_path.exists(),
            "old file must remain as a safety copy"
        );
    }

    #[test]
    fn rejects_removed_legacy_fields() {
        let bytes = serde_json::to_vec(&json!({
            "schema_version": 2,
            "progress": {},
            "library": {},
            "bookmarks": [42]
        }))
        .expect("serialize legacy field");

        assert!(matches!(
            parse_history(&bytes),
            Err(ParseHistoryError::Invalid { .. })
        ));
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
            library: HashMap::new(),
        };
        let newer = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(2, 1, 1, "Dub"),
                progress(2, "Newer", 1, 1, "Dub", true, 2),
            )]),
            library: HashMap::new(),
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
            library: HashMap::new(),
        };
        let second = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(4, 1, 1, "Dub"),
                progress(4, "Second", 1, 1, "Dub", true, 4),
            )]),
            library: HashMap::new(),
        };

        manager.save_history(&first).expect("save first");
        let first_on_disk = fs::read(directory.history_path()).expect("read first");
        let first_json: Value = serde_json::from_slice(&first_on_disk).expect("parse first");
        assert_eq!(first_json["schema_version"], json!(2));

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
    fn batch_episode_updates_share_one_transaction() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let initial = AppHistory {
            progress: HashMap::from([(
                StorageManager::make_progress_key(11, 1, 1, "Dub"),
                progress(11, "Initial", 1, 1, "Dub", false, 1),
            )]),
            library: HashMap::new(),
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

    #[test]
    fn multiple_releases_and_episodes_share_one_transaction() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let statuses = [11, 12].map(|anime_id| AnimeStatusUpdate {
            anime_id,
            title: "Франшиза".to_string(),
            status: AnimeStatus::Completed,
            release: Some(LibraryReleaseMetadata {
                title: format!("Сезон {}", anime_id - 10),
                kind: LibraryReleaseKind::Season,
                season: anime_id - 10,
                part: Some(1),
                episodes_count: Some(1),
                first_episode: Some(1),
                airing_status: None,
                next_airing_episode: None,
                next_airing_at: None,
            }),
        });
        let episodes = [11, 12].map(|anime_id| EpisodeWatchedUpdate {
            anime_id,
            anime_title: "Франшиза".to_string(),
            season: anime_id - 10,
            episode: 1,
            studio_name: "Статус".to_string(),
            watched: true,
        });

        let history = manager
            .set_releases_watched(&statuses, &episodes)
            .expect("mark franchise watched");

        assert_eq!(history.library.len(), 2);
        assert_eq!(history.progress.len(), 2);
        assert!(
            history
                .library
                .values()
                .all(|record| record.status == AnimeStatus::Completed)
        );
        assert!(history.progress.values().all(|progress| progress.watched));
    }

    #[test]
    fn anime_status_is_persisted_for_every_release() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        let history = manager
            .set_anime_status(&[42, 43], "Каґуя", AnimeStatus::Watching)
            .expect("set anime status");

        assert_eq!(history.library[&42].status, AnimeStatus::Watching);
        assert_eq!(history.library[&43].title, "Каґуя");
        assert_eq!(manager.load_history().expect("reload history"), history);
    }

    #[test]
    fn clear_library_removes_statuses_and_progress_together() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        manager
            .set_anime_status(&[42], "Каґуя", AnimeStatus::Watching)
            .expect("seed status");
        manager
            .set_episode_watched(42, "Каґуя", 1, 1, "Dub", true)
            .expect("seed progress");

        let history = manager.clear_library().expect("clear library");
        assert!(history.library.is_empty());
        assert!(history.progress.is_empty());
        assert_eq!(
            manager.load_history().expect("reload cleared history"),
            history
        );
    }

    #[test]
    fn manual_episode_toggle_syncs_moonanime_with_other_dubbings() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        manager
            .set_episode_watched(42, "Каґуя", 3, 4, "Ashdi", true)
            .expect("seed Ashdi progress");

        let history = manager
            .set_episode_watched_across_dubbings(42, "Каґуя", 3, 4, "MoonAnime", false)
            .expect("unwatch through MoonAnime");
        let matching = history
            .progress
            .values()
            .filter(|progress| {
                progress.anime_id == 42 && progress.season == 3 && progress.episode == 4
            })
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 2);
        assert!(matching.iter().all(|progress| !progress.watched));

        let history = manager
            .set_episode_watched_across_dubbings(42, "Каґуя", 3, 4, "MoonAnime", true)
            .expect("watch through MoonAnime");
        assert!(
            history
                .progress
                .values()
                .filter(|progress| {
                    progress.anime_id == 42 && progress.season == 3 && progress.episode == 4
                })
                .all(|progress| progress.watched)
        );
    }

    #[test]
    fn watched_threshold_can_be_configured_or_disabled() {
        assert!(!StorageManager::compute_watched(899.0, 1000.0, Some(90)));
        assert!(StorageManager::compute_watched(900.0, 1000.0, Some(90)));
        assert!(!StorageManager::compute_watched(1000.0, 1000.0, None));
        assert!(!StorageManager::compute_watched(1200.0, 0.0, Some(90)));
    }

    #[test]
    fn clearing_timestamp_preserves_watched_state() {
        let directory = TestDirectory::new();
        let manager = directory.manager();
        manager
            .update_history_batch(|history| {
                history.progress.insert(
                    StorageManager::make_progress_key(7, 2, 3, "Dub"),
                    WatchProgress {
                        anime_id: 7,
                        anime_title: "Test".to_string(),
                        season: 2,
                        episode: 3,
                        studio_name: "Dub".to_string(),
                        timestamp: 777.0,
                        duration: 1400.0,
                        watched: true,
                        updated_at: 1,
                    },
                );
            })
            .expect("seed progress");

        let history = manager
            .clear_episode_timestamp(7, 2, 3, "Dub")
            .expect("clear timestamp");
        let progress = &history.progress[&StorageManager::make_progress_key(7, 2, 3, "Dub")];
        assert_eq!(progress.timestamp, 0.0);
        assert!(progress.watched);
    }
}
