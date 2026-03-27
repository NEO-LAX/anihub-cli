use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WatchProgress {
    pub anime_id: u32,
    pub anime_title: String,
    pub season: u32,
    pub episode: u32,
    #[serde(default)]
    pub timestamp: f64, // час у секундах, на якому зупинився користувач
    #[serde(default)]
    pub duration: f64, // загальна тривалість епізоду, якщо відома
    #[serde(default)]
    pub watched: bool, // чи вважається серія переглянутою
    #[serde(default)]
    pub updated_at: i64, // Unix timestamp для сортування "Продовжити перегляд"
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct AppHistory {
    /// Ключ - це ID аніме. Значення - прогрес перегляду.
    pub progress: HashMap<String, WatchProgress>,
    pub bookmarks: Vec<u32>, // Список ID аніме, які збережені в закладки
}

pub struct StorageManager {
    history_file: PathBuf,
}

impl StorageManager {
    pub fn make_progress_key(anime_id: u32, season: u32, episode: u32) -> String {
        format!("{anime_id}:{season}:{episode}")
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
        if !self.history_file.exists() {
            // Якщо файлу ще немає, повертаємо порожню історію
            return Ok(AppHistory::default());
        }

        let content =
            fs::read_to_string(&self.history_file).context("Failed to read history file")?;

        let mut history: AppHistory =
            serde_json::from_str(&content).unwrap_or_else(|_| AppHistory::default()); // При помилці парсингу повертаємо дефолт

        // Міграція старих ключів `anime_id` -> `anime_id:season:episode`.
        let migrated = history
            .progress
            .into_values()
            .map(|mut progress| {
                progress.watched = Self::compute_watched(progress.timestamp, progress.duration);
                (
                    Self::make_progress_key(progress.anime_id, progress.season, progress.episode),
                    progress,
                )
            })
            .collect();
        history.progress = migrated;

        Ok(history)
    }

    pub fn save_history(&self, history: &AppHistory) -> Result<()> {
        let content =
            serde_json::to_string_pretty(history).context("Failed to serialize history")?;

        fs::write(&self.history_file, content).context("Failed to write history file")?;

        Ok(())
    }

    pub fn compute_watched(timestamp: f64, duration: f64) -> bool {
        if duration > 0.0 {
            (timestamp / duration >= 0.80) || (timestamp >= 1200.0)
        } else {
            timestamp >= 1200.0
        }
    }

    pub fn update_progress(
        &self,
        anime_id: u32,
        title: &str,
        season: u32,
        episode: u32,
        timestamp: f64,
        duration: f64,
    ) -> Result<()> {
        let mut history = self.load_history()?;

        let progress = WatchProgress {
            anime_id,
            anime_title: title.to_string(),
            season,
            episode,
            timestamp,
            duration,
            watched: Self::compute_watched(timestamp, duration),
            updated_at: chrono::Utc::now().timestamp(),
        };

        history
            .progress
            .insert(Self::make_progress_key(anime_id, season, episode), progress);
        self.save_history(&history)?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_anime_progress(&self, anime_id: u32) -> Result<()> {
        let mut history = self.load_history()?;
        history
            .progress
            .retain(|_, progress| progress.anime_id != anime_id);
        self.save_history(&history)
    }

    pub fn delete_anime_progresses(&self, anime_ids: &[u32]) -> Result<()> {
        let mut history = self.load_history()?;
        history
            .progress
            .retain(|_, progress| !anime_ids.contains(&progress.anime_id));
        self.save_history(&history)
    }

    #[allow(dead_code)]
    pub fn reset_season_watched(&self, anime_id: u32, season: u32) -> Result<()> {
        let mut history = self.load_history()?;
        for progress in history.progress.values_mut() {
            if progress.anime_id == anime_id && progress.season == season {
                progress.watched = false;
                progress.updated_at = chrono::Utc::now().timestamp();
            }
        }
        self.save_history(&history)
    }

    #[allow(dead_code)]
    pub fn reset_episode_progress(&self, anime_id: u32, season: u32, episode: u32) -> Result<()> {
        let mut history = self.load_history()?;
        if let Some(progress) = history.progress.values_mut().find(|progress| {
            progress.anime_id == anime_id
                && progress.season == season
                && progress.episode == episode
        }) {
            if progress.watched {
                progress.watched = false;
            } else if progress.timestamp > 0.0 {
                progress.timestamp = 0.0;
            }
            progress.updated_at = chrono::Utc::now().timestamp();
        }
        self.save_history(&history)
    }

    pub fn set_episode_watched(
        &self,
        anime_id: u32,
        title: &str,
        season: u32,
        episode: u32,
        watched: bool,
    ) -> Result<()> {
        let mut history = self.load_history()?;
        let key = Self::make_progress_key(anime_id, season, episode);
        let now = chrono::Utc::now().timestamp();

        match history.progress.get_mut(&key) {
            Some(progress) => {
                progress.anime_title = title.to_string();
                progress.watched = watched;
                if watched {
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
                        anime_id,
                        anime_title: title.to_string(),
                        season,
                        episode,
                        timestamp: if watched { 1200.0 } else { 0.0 },
                        duration: 0.0,
                        watched,
                        updated_at: now,
                    },
                );
            }
        }

        self.save_history(&history)
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
        let mut history = self.load_history()?;
        let mut is_bookmarked = false;

        if let Some(pos) = history.bookmarks.iter().position(|&x| x == anime_id) {
            history.bookmarks.remove(pos); // Якщо вже є - видаляємо (зняти закладку)
        } else {
            history.bookmarks.push(anime_id); // Якщо немає - додаємо
            is_bookmarked = true;
        }

        self.save_history(&history)?;
        Ok(is_bookmarked)
    }
}
