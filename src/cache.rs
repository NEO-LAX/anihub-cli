use crate::api::{AniListMedia, AnimeDetails, AnimeItem};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_FILE_NAME: &str = "metadata-cache.json";
const CACHE_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const DETAILS_FRESHNESS: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_SEARCHES: usize = 64;
const MAX_DETAILS: usize = 500;
const WRITE_DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CachedSearch {
    pub items: Vec<AnimeItem>,
    pub anilist_media: Vec<AniListMedia>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedDetails {
    value: AnimeDetails,
    updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CacheEnvelope {
    schema_version: u32,
    searches: HashMap<String, CachedSearch>,
    details: HashMap<u32, CachedDetails>,
}

impl Default for CacheEnvelope {
    fn default() -> Self {
        Self {
            schema_version: CACHE_SCHEMA_VERSION,
            searches: HashMap::new(),
            details: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct MetadataCache {
    path: PathBuf,
    data: CacheEnvelope,
    writer: CacheWriter,
}

#[derive(Debug)]
struct CacheWriter {
    sender: mpsc::Sender<WriterCommand>,
    thread: Option<thread::JoinHandle<()>>,
}

#[derive(Debug)]
enum WriterCommand {
    PutSearch(String, CachedSearch),
    PutDetails(u32, Box<CachedDetails>),
    Flush(mpsc::Sender<std::result::Result<(), String>>),
    Shutdown(mpsc::Sender<std::result::Result<(), String>>),
}

impl MetadataCache {
    pub fn new(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(CACHE_FILE_NAME);
        let mut data = if path.exists() {
            match fs::read(&path)
                .with_context(|| format!("не вдалося прочитати {}", path.display()))
                .and_then(|bytes| {
                    serde_json::from_slice::<CacheEnvelope>(&bytes)
                        .with_context(|| format!("пошкоджено {}", path.display()))
                }) {
                Ok(data) if data.schema_version == CACHE_SCHEMA_VERSION => data,
                Ok(_) | Err(_) => {
                    preserve_corrupt_cache(&path);
                    CacheEnvelope::default()
                }
            }
        } else {
            CacheEnvelope::default()
        };
        prune(&mut data);
        let writer = CacheWriter::spawn(path.clone(), data.clone());
        Ok(Self { path, data, writer })
    }

    pub fn search(&self, query: &str, extended: bool) -> Option<CachedSearch> {
        self.data
            .searches
            .get(&search_key(query, extended))
            .cloned()
    }

    /// Iterate over persistent search payloads so callers can reconstruct a
    /// franchise that was already discovered before the current process.
    pub fn searches(&self) -> impl Iterator<Item = &CachedSearch> {
        self.data.searches.values()
    }

    pub fn put_search(
        &mut self,
        query: &str,
        extended: bool,
        items: Vec<AnimeItem>,
        anilist_media: Vec<AniListMedia>,
    ) -> Result<()> {
        let key = search_key(query, extended);
        let cached = CachedSearch {
            items,
            anilist_media,
            updated_at: Utc::now().timestamp(),
        };
        self.data.searches.insert(key.clone(), cached.clone());
        prune(&mut self.data);
        self.writer.send(WriterCommand::PutSearch(key, cached))
    }

    pub fn put_details(&mut self, details: AnimeDetails) -> Result<()> {
        let anime_id = details.id;
        let cached = CachedDetails {
            value: details,
            updated_at: Utc::now().timestamp(),
        };
        self.data.details.insert(anime_id, cached.clone());
        prune(&mut self.data);
        self.writer
            .send(WriterCommand::PutDetails(anime_id, Box::new(cached)))
    }

    pub fn details(&self) -> impl Iterator<Item = (u32, AnimeDetails)> + '_ {
        self.data
            .details
            .iter()
            .map(|(&anime_id, cached)| (anime_id, cached.value.clone()))
    }

    pub fn details_are_fresh(&self, anime_id: u32) -> bool {
        self.data
            .details
            .get(&anime_id)
            .is_some_and(|cached| age_seconds(cached.updated_at) <= DETAILS_FRESHNESS.as_secs())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn flush(&self) -> Result<()> {
        self.writer.flush()
    }
}

impl CacheWriter {
    fn spawn(path: PathBuf, data: CacheEnvelope) -> Self {
        let (sender, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("metadata-cache-writer".to_string())
            .spawn(move || writer_loop(&path, data, &receiver))
            .expect("failed to start metadata cache writer");
        Self {
            sender,
            thread: Some(thread),
        }
    }

    fn send(&self, command: WriterCommand) -> Result<()> {
        self.sender
            .send(command)
            .map_err(|_| anyhow::anyhow!("фоновий запис кешу несподівано зупинився"))
    }

    fn flush(&self) -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        self.send(WriterCommand::Flush(sender))?;
        receiver
            .recv()
            .map_err(|_| anyhow::anyhow!("фоновий запис кешу несподівано зупинився"))?
            .map_err(anyhow::Error::msg)
    }
}

impl Drop for CacheWriter {
    fn drop(&mut self) {
        let (sender, receiver) = mpsc::channel();
        if self.sender.send(WriterCommand::Shutdown(sender)).is_ok() {
            let _ = receiver.recv();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn writer_loop(path: &Path, mut data: CacheEnvelope, receiver: &mpsc::Receiver<WriterCommand>) {
    let mut dirty = false;
    loop {
        let command = if dirty {
            match receiver.recv_timeout(WRITE_DEBOUNCE) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if write_cache(path, &data).is_ok() {
                        dirty = false;
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = write_cache(path, &data);
                    break;
                }
            }
        } else {
            match receiver.recv() {
                Ok(command) => command,
                Err(_) => break,
            }
        };

        match command {
            WriterCommand::PutSearch(key, cached) => {
                data.searches.insert(key, cached);
                prune(&mut data);
                dirty = true;
            }
            WriterCommand::PutDetails(anime_id, cached) => {
                data.details.insert(anime_id, *cached);
                prune(&mut data);
                dirty = true;
            }
            WriterCommand::Flush(reply) => {
                let result = if dirty {
                    write_cache(path, &data).map_err(|error| error.to_string())
                } else {
                    Ok(())
                };
                if result.is_ok() {
                    dirty = false;
                }
                let _ = reply.send(result);
            }
            WriterCommand::Shutdown(reply) => {
                let result = if dirty {
                    write_cache(path, &data).map_err(|error| error.to_string())
                } else {
                    Ok(())
                };
                let _ = reply.send(result);
                break;
            }
        }
    }
}

fn write_cache(path: &Path, data: &CacheEnvelope) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(data)?;
    crate::atomic_file::write(path, &bytes)
}

fn search_key(query: &str, extended: bool) -> String {
    let mode = if extended { "extended" } else { "strict" };
    format!("{mode}:{}", normalize_query(query))
}

fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn age_seconds(updated_at: i64) -> u64 {
    Utc::now().timestamp().saturating_sub(updated_at).max(0) as u64
}

fn prune(data: &mut CacheEnvelope) {
    let retention = CACHE_RETENTION.as_secs();
    data.searches
        .retain(|_, cached| age_seconds(cached.updated_at) <= retention);
    data.details
        .retain(|_, cached| age_seconds(cached.updated_at) <= retention);
    retain_newest(&mut data.searches, MAX_SEARCHES, |cached| cached.updated_at);
    retain_newest(&mut data.details, MAX_DETAILS, |cached| cached.updated_at);
}

fn retain_newest<K, V, F>(entries: &mut HashMap<K, V>, limit: usize, timestamp: F)
where
    K: Clone + Eq + std::hash::Hash,
    F: Fn(&V) -> i64,
{
    if entries.len() <= limit {
        return;
    }
    let mut oldest = entries
        .iter()
        .map(|(key, value)| (key.clone(), timestamp(value)))
        .collect::<Vec<_>>();
    oldest.sort_by_key(|(_, updated_at)| *updated_at);
    for (key, _) in oldest.into_iter().take(entries.len() - limit) {
        entries.remove(&key);
    }
}

fn preserve_corrupt_cache(path: &Path) {
    let preserved = path.with_extension(format!("corrupt-{}", Utc::now().timestamp()));
    let _ = fs::rename(path, preserved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("anihub-cache-{label}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn item(id: u32) -> AnimeItem {
        AnimeItem {
            id,
            anilist_id: Some(id + 1000),
            slug: format!("anime-{id}"),
            title_ukrainian: format!("Аніме {id}"),
            title_original: None,
            title_english: None,
            status: "ongoing".to_string(),
            anime_type: "tv".to_string(),
            year: Some(2026),
            has_ukrainian_dub: true,
            poster_url: None,
            episodes_count: Some(12),
            description: None,
            rating: None,
            genres: None,
            dubbing_studios: None,
        }
    }

    #[test]
    fn search_modes_have_separate_persistent_entries() {
        let directory = temp_dir("modes");
        let mut cache = MetadataCache::new(&directory).unwrap();
        cache
            .put_search("  Каґуя  ", false, vec![item(1)], Vec::new())
            .unwrap();
        cache
            .put_search("каґуя", true, vec![item(2)], Vec::new())
            .unwrap();
        cache.flush().unwrap();

        let cache = MetadataCache::new(&directory).unwrap();
        assert_eq!(cache.search("КАҐУЯ", false).unwrap().items[0].id, 1);
        assert_eq!(cache.search("каґуя", true).unwrap().items[0].id, 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_cache_is_preserved_and_replaced_with_empty_state() {
        let directory = temp_dir("corrupt");
        let path = directory.join(CACHE_FILE_NAME);
        fs::write(&path, b"{ broken").unwrap();

        let cache = MetadataCache::new(&directory).unwrap();
        assert!(cache.search("test", false).is_none());
        assert!(fs::read_dir(&directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("corrupt")
        }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn foreground_updates_memory_before_debounced_flush() {
        let directory = temp_dir("foreground");
        let mut cache = MetadataCache::new(&directory).unwrap();

        cache
            .put_search("frieren", false, vec![item(7)], Vec::new())
            .unwrap();

        assert_eq!(cache.search("frieren", false).unwrap().items[0].id, 7);
        cache.flush().unwrap();
        assert!(directory.join(CACHE_FILE_NAME).exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
