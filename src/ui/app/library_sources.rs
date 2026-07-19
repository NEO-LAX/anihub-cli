//! Release-local source summaries used by the Library list.

use super::*;

impl AppState {
    /// Return a dubbing count only when sources for this exact AniHub release
    /// have been loaded. Split cours can share a conceptual season number, so
    /// `anime_id` must remain part of the cache key.
    pub fn library_release_dubbing_count(&self, release: &LibrarySeasonEntry) -> Option<usize> {
        release_dubbing_count(&self.sources_cache, release)
    }
}

fn release_dubbing_count(
    cache: &moka::sync::Cache<EpisodeSourcesKey, EpisodeSourcesResponse>,
    release: &LibrarySeasonEntry,
) -> Option<usize> {
    let key = EpisodeSourcesKey::new(release.anime_id, release.season);
    let sources = cache.get(&key)?;
    Some(dubbing_choices_for_sources(&sources, release.season).len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(anime_id: u32, part: u32) -> LibrarySeasonEntry {
        LibrarySeasonEntry {
            anime_id,
            season: 1,
            part: Some(part),
            title: format!("Частина {part}"),
            kind: LibraryReleaseKind::Season,
            episodes_count: Some(12),
            first_episode: Some(1),
            airing_status: None,
            next_airing_episode: None,
            next_airing_at: None,
            status: AnimeStatus::Watching,
            episodes: Vec::new(),
        }
    }

    fn sources(anime_id: u32, count: usize) -> (EpisodeSourcesKey, EpisodeSourcesResponse) {
        (
            EpisodeSourcesKey::new(anime_id, 1),
            EpisodeSourcesResponse {
                ashdi: (0..count)
                    .map(|index| AshdiStudio {
                        id: index as u32,
                        studio_name: format!("Dub {index}"),
                        season_number: 1,
                        episodes: Vec::new(),
                        episodes_count: 12,
                    })
                    .collect(),
                moonanime: Vec::new(),
            },
        )
    }

    #[test]
    fn split_cours_use_their_own_release_source_counts() {
        let cache = moka::sync::Cache::new(8);
        let first = release(10, 1);
        let second = release(20, 2);
        let (first_key, first_sources) = sources(first.anime_id, 5);
        let (second_key, second_sources) = sources(second.anime_id, 2);
        cache.insert(first_key, first_sources);
        cache.insert(second_key, second_sources);

        assert_eq!(release_dubbing_count(&cache, &first), Some(5));
        assert_eq!(release_dubbing_count(&cache, &second), Some(2));
    }

    #[test]
    fn unloaded_release_does_not_borrow_the_selected_release_count() {
        let cache = moka::sync::Cache::new(8);
        let first = release(10, 1);
        let second = release(20, 2);
        let (first_key, first_sources) = sources(first.anime_id, 5);
        cache.insert(first_key, first_sources);

        assert_eq!(release_dubbing_count(&cache, &first), Some(5));
        assert_eq!(release_dubbing_count(&cache, &second), None);
    }
}
