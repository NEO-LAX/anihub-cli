//! Release-local source summaries used by the Library list.

use super::*;

impl AppState {
    /// Return a dubbing count only when sources for this exact AniHub release
    /// have been loaded. Split cours can share a conceptual season number, so
    /// `anime_id` must remain part of the cache key.
    pub fn library_release_dubbing_count(&self, release: &LibrarySeasonEntry) -> Option<usize> {
        let selected_season = self.library_selected_season().map(|release| release.season);
        release_dubbing_count(&self.sources_cache, release, selected_season)
    }
}

fn release_dubbing_count(
    cache: &moka::sync::Cache<EpisodeSourcesKey, EpisodeSourcesResponse>,
    release: &LibrarySeasonEntry,
    selected_season: Option<u32>,
) -> Option<usize> {
    if selected_season != Some(release.season) {
        return None;
    }
    let key = EpisodeSourcesKey::new(release.anime_id, release.season);
    let sources = cache.get(&key)?;
    Some(dubbing_choices_for_sources(&sources, release.season).len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(anime_id: u32, season: u32, part: u32) -> LibrarySeasonEntry {
        LibrarySeasonEntry {
            anime_id,
            season,
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

    fn sources(
        anime_id: u32,
        season: u32,
        count: usize,
    ) -> (EpisodeSourcesKey, EpisodeSourcesResponse) {
        (
            EpisodeSourcesKey::new(anime_id, season),
            EpisodeSourcesResponse {
                ashdi: (0..count)
                    .map(|index| AshdiStudio {
                        id: index as u32,
                        studio_name: format!("Dub {index}"),
                        season_number: season,
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
        let first = release(10, 1, 1);
        let second = release(20, 1, 2);
        let (first_key, first_sources) = sources(first.anime_id, first.season, 5);
        let (second_key, second_sources) = sources(second.anime_id, second.season, 2);
        cache.insert(first_key, first_sources);
        cache.insert(second_key, second_sources);

        assert_eq!(release_dubbing_count(&cache, &first, Some(1)), Some(5));
        assert_eq!(release_dubbing_count(&cache, &second, Some(1)), Some(2));
    }

    #[test]
    fn unloaded_release_does_not_borrow_the_selected_release_count() {
        let cache = moka::sync::Cache::new(8);
        let first = release(10, 1, 1);
        let second = release(20, 1, 2);
        let (first_key, first_sources) = sources(first.anime_id, first.season, 5);
        cache.insert(first_key, first_sources);

        assert_eq!(release_dubbing_count(&cache, &first, Some(1)), Some(5));
        assert_eq!(release_dubbing_count(&cache, &second, Some(1)), None);
    }

    #[test]
    fn counts_are_visible_only_for_parts_of_the_selected_season() {
        let cache = moka::sync::Cache::new(8);
        let first = release(10, 1, 1);
        let second = release(20, 1, 2);
        let next_season = release(30, 2, 1);
        for release in [&first, &second, &next_season] {
            let count = if release.season == 1 { 2 } else { 4 };
            let (key, value) = sources(release.anime_id, release.season, count);
            cache.insert(key, value);
        }

        assert_eq!(release_dubbing_count(&cache, &first, Some(1)), Some(2));
        assert_eq!(release_dubbing_count(&cache, &second, Some(1)), Some(2));
        assert_eq!(release_dubbing_count(&cache, &next_season, Some(1)), None);
        assert_eq!(release_dubbing_count(&cache, &first, Some(2)), None);
        assert_eq!(
            release_dubbing_count(&cache, &next_season, Some(2)),
            Some(4)
        );
    }
}
