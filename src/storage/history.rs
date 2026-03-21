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
    pub timestamp: f64, // час у секундах, на якому зупинився користувач
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

        let content = fs::read_to_string(&self.history_file)
            .context("Failed to read history file")?;
        
        let history: AppHistory = serde_json::from_str(&content)
            .unwrap_or_else(|_| AppHistory::default()); // При помилці парсингу повертаємо дефолт

        Ok(history)
    }

    pub fn save_history(&self, history: &AppHistory) -> Result<()> {
        let content = serde_json::to_string_pretty(history)
            .context("Failed to serialize history")?;
        
        fs::write(&self.history_file, content)
            .context("Failed to write history file")?;
            
        Ok(())
    }

    pub fn update_progress(&self, anime_id: u32, title: &str, season: u32, episode: u32, timestamp: f64) -> Result<()> {
        let mut history = self.load_history()?;
        
        let progress = WatchProgress {
            anime_id,
            anime_title: title.to_string(),
            season,
            episode,
            timestamp,
            updated_at: chrono::Utc::now().timestamp(),
        };

        history.progress.insert(anime_id.to_string(), progress);
        self.save_history(&history)?;

        Ok(())
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
