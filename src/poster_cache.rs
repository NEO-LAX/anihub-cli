use anyhow::{Context, Result};
use image::DynamicImage;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const DEFAULT_MAX_BYTES: u64 = 150 * 1024 * 1024;

/// Disposable on-disk cache for original poster response bytes.
///
/// Watch history and settings never live here, so corrupt or old entries can
/// be removed without risking user data.
pub struct PosterCache {
    directory: PathBuf,
    max_bytes: u64,
}

impl PosterCache {
    pub fn new(data_dir: &Path) -> Result<Self> {
        Self::with_limit(data_dir.join("posters"), DEFAULT_MAX_BYTES)
    }

    fn with_limit(directory: PathBuf, max_bytes: u64) -> Result<Self> {
        fs::create_dir_all(&directory)
            .with_context(|| format!("не вдалося створити {}", directory.display()))?;
        let cache = Self {
            directory,
            max_bytes: max_bytes.max(1),
        };
        cache.prune()?;
        Ok(cache)
    }

    pub fn path(&self) -> &Path {
        &self.directory
    }

    pub fn load(&self, url: &str) -> Result<Option<DynamicImage>> {
        let path = self.entry_path(url);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("не вдалося прочитати {}", path.display()));
            }
        };

        match image::load_from_memory(&bytes) {
            Ok(image) => Ok(Some(image)),
            Err(_) => {
                // Poster cache is disposable. A broken image must never block
                // the network fallback or poison subsequent launches.
                let _ = fs::remove_file(path);
                Ok(None)
            }
        }
    }

    pub fn store(&self, url: &str, bytes: &[u8]) -> Result<()> {
        fs::create_dir_all(&self.directory)
            .with_context(|| format!("не вдалося створити {}", self.directory.display()))?;
        let destination = self.entry_path(url);
        let temporary = destination.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temporary, bytes)
            .with_context(|| format!("не вдалося записати {}", temporary.display()))?;
        if let Err(first_error) = fs::rename(&temporary, &destination) {
            if destination.exists() {
                fs::remove_file(&destination)
                    .with_context(|| format!("не вдалося замінити {}", destination.display()))?;
                fs::rename(&temporary, &destination)
                    .with_context(|| format!("не вдалося оновити {}", destination.display()))?;
            } else {
                return Err(first_error)
                    .with_context(|| format!("не вдалося зберегти {}", destination.display()));
            }
        }
        self.prune()
    }

    pub fn clear(&self) -> Result<()> {
        match fs::remove_dir_all(&self.directory) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("не вдалося очистити {}", self.directory.display()));
            }
        }
        fs::create_dir_all(&self.directory)
            .with_context(|| format!("не вдалося створити {}", self.directory.display()))
    }

    fn entry_path(&self, url: &str) -> PathBuf {
        let digest = Sha256::digest(url.as_bytes());
        self.directory.join(format!("{}.img", hex::encode(digest)))
    }

    fn prune(&self) -> Result<()> {
        let mut entries = Vec::new();
        let mut total = 0u64;
        for entry in fs::read_dir(&self.directory)
            .with_context(|| format!("не вдалося прочитати {}", self.directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("img") {
                continue;
            }
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                continue;
            }
            let size = metadata.len();
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            total = total.saturating_add(size);
            entries.push((modified, path, size));
        }
        entries.sort_by_key(|(modified, path, _)| (*modified, path.clone()));
        for (_, path, size) in entries {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};
    use std::io::Cursor;
    use std::time::{Duration, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "anihub-poster-cache-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    fn png_bytes() -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(2, 3));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn cached_poster_round_trips_and_clear_is_safe() {
        let directory = temporary_directory("round-trip");
        let cache = PosterCache::with_limit(directory.clone(), 1024 * 1024).unwrap();
        cache
            .store("https://example.test/poster.jpg", &png_bytes())
            .unwrap();

        let loaded = cache
            .load("https://example.test/poster.jpg")
            .unwrap()
            .unwrap();
        assert_eq!((loaded.width(), loaded.height()), (2, 3));

        cache.clear().unwrap();
        assert!(
            cache
                .load("https://example.test/poster.jpg")
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_entry_is_removed_and_becomes_a_cache_miss() {
        let directory = temporary_directory("corrupt");
        let cache = PosterCache::with_limit(directory.clone(), 1024).unwrap();
        let path = cache.entry_path("https://example.test/broken.jpg");
        fs::write(&path, b"not an image").unwrap();

        assert!(
            cache
                .load("https://example.test/broken.jpg")
                .unwrap()
                .is_none()
        );
        assert!(!path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn oldest_posters_are_pruned_to_the_size_limit() {
        let directory = temporary_directory("prune");
        let bytes = png_bytes();
        let cache = PosterCache::with_limit(directory.clone(), bytes.len() as u64 + 8).unwrap();
        cache.store("https://example.test/one.jpg", &bytes).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        cache.store("https://example.test/two.jpg", &bytes).unwrap();

        assert!(
            cache
                .load("https://example.test/one.jpg")
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .load("https://example.test/two.jpg")
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
