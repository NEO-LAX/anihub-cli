//! Translation between AniList/AniHub franchise catalogs and persisted library
//! records. These functions are pure: storage writes and UI refreshes stay in
//! `library_sync`.

use crate::api::{
    AniListMedia, AnimeItem, FranchiseCatalog, ReleaseAvailability, ReleaseClassification,
    ReleaseEntry,
};
use crate::cache::MetadataCache;
use crate::storage::{
    AnimeStatus, AnimeStatusUpdate, AppHistory, LibraryReleaseKind, LibraryReleaseMetadata,
};
use std::collections::{BTreeMap, HashSet};

pub(super) fn cached_franchise_catalogs_for_history(
    cache: &MetadataCache,
    history: &AppHistory,
) -> Vec<FranchiseCatalog> {
    let history_ids = history
        .progress
        .values()
        .map(|progress| progress.anime_id)
        .chain(history.library.keys().copied())
        .collect::<HashSet<_>>();
    if history_ids.is_empty() {
        return Vec::new();
    }

    let mut items = BTreeMap::<u32, AnimeItem>::new();
    let mut media = BTreeMap::<u32, AniListMedia>::new();
    for cached in cache.searches().filter(|cached| {
        cached
            .items
            .iter()
            .any(|item| history_ids.contains(&item.id))
    }) {
        for item in &cached.items {
            items.entry(item.id).or_insert_with(|| item.clone());
        }
        for node in &cached.anilist_media {
            media.entry(node.id).or_insert_with(|| node.clone());
        }
    }

    crate::api::build_franchise_catalogs(
        &items.into_values().collect::<Vec<_>>(),
        &media.into_values().collect::<Vec<_>>(),
    )
    .into_iter()
    .filter(|catalog| {
        catalog.releases.iter().any(|release| {
            release
                .anihub_id
                .is_some_and(|anime_id| history_ids.contains(&anime_id))
        })
    })
    .collect()
}

pub(super) fn library_metadata_for_release(
    catalog: &FranchiseCatalog,
    release: &ReleaseEntry,
) -> LibraryReleaseMetadata {
    let kind = match release.classification {
        ReleaseClassification::MainlineSeason => LibraryReleaseKind::Season,
        ReleaseClassification::MainlineMovie => LibraryReleaseKind::Movie,
        ReleaseClassification::MainlineSpecial => LibraryReleaseKind::Special,
        ReleaseClassification::Extra => LibraryReleaseKind::Extra,
    };
    let part = release.part.unwrap_or(1);
    let offset = catalog
        .releases
        .iter()
        .filter(|candidate| {
            candidate.classification == release.classification
                && candidate.conceptual_season == release.conceptual_season
                && candidate.part.unwrap_or(1) < part
        })
        .filter_map(|candidate| candidate.available_episodes.or(candidate.episodes_count))
        .sum::<u32>();
    LibraryReleaseMetadata {
        title: release.title.clone(),
        kind,
        season: release.conceptual_season.unwrap_or(1),
        part: release.part,
        episodes_count: release.available_episodes.or(release.episodes_count),
        first_episode: Some(offset.saturating_add(1)),
        airing_status: release.airing_status.clone(),
        next_airing_episode: release.next_airing_episode,
        next_airing_at: release.next_airing_at,
    }
}

pub(super) fn library_catalog_updates(
    history: &AppHistory,
    catalogs: &[FranchiseCatalog],
) -> Vec<AnimeStatusUpdate> {
    let mut updates = BTreeMap::<u32, AnimeStatusUpdate>::new();

    for catalog in catalogs {
        let available = catalog
            .releases
            .iter()
            .filter(|release| release.availability == ReleaseAvailability::Available)
            .filter_map(|release| release.anihub_id.map(|anime_id| (anime_id, release)))
            .collect::<Vec<_>>();
        if available.is_empty() {
            continue;
        }

        let has_progress = available.iter().any(|(anime_id, _)| {
            history
                .progress
                .values()
                .any(|progress| progress.anime_id == *anime_id)
        });
        let records = available
            .iter()
            .filter_map(|(anime_id, _)| history.library.get(anime_id))
            .collect::<Vec<_>>();
        let has_active_record = records
            .iter()
            .any(|record| record.status != AnimeStatus::NotAdded);
        let explicitly_removed = !has_active_record
            && records
                .iter()
                .any(|record| record.status == AnimeStatus::NotAdded);
        if explicitly_removed || (!has_progress && !has_active_record) {
            continue;
        }

        for (anime_id, release) in available {
            let metadata = library_metadata_for_release(catalog, release);
            let existing = history.library.get(&anime_id);
            let status = match existing {
                Some(record)
                    if record.status == AnimeStatus::Completed
                        && release_metadata_is_ongoing(&metadata) =>
                {
                    AnimeStatus::Watching
                }
                Some(record) => record.status,
                None => inferred_release_status(history, anime_id, &metadata),
            };
            if existing.is_some_and(|record| {
                record.title == catalog.canonical_title
                    && record.status == status
                    && record.release.as_ref() == Some(&metadata)
            }) {
                continue;
            }
            updates.insert(
                anime_id,
                AnimeStatusUpdate {
                    anime_id,
                    title: catalog.canonical_title.clone(),
                    status,
                    release: Some(metadata),
                },
            );
        }
    }

    updates.into_values().collect()
}

pub(super) fn inferred_release_status(
    history: &AppHistory,
    anime_id: u32,
    metadata: &LibraryReleaseMetadata,
) -> AnimeStatus {
    let progress = history
        .progress
        .values()
        .filter(|progress| progress.anime_id == anime_id)
        .collect::<Vec<_>>();
    if progress.is_empty() {
        return AnimeStatus::NotAdded;
    }

    let watched = progress
        .iter()
        .filter(|progress| progress.watched)
        .map(|progress| progress.episode)
        .collect::<HashSet<_>>()
        .len() as u32;
    if release_metadata_is_ongoing(metadata) {
        AnimeStatus::Watching
    } else if metadata
        .episodes_count
        .is_some_and(|episodes| episodes > 0 && watched >= episodes)
    {
        AnimeStatus::Completed
    } else {
        AnimeStatus::Watching
    }
}

pub(super) fn release_metadata_is_ongoing(metadata: &LibraryReleaseMetadata) -> bool {
    metadata.next_airing_episode.is_some()
        || metadata
            .airing_status
            .as_deref()
            .is_some_and(|status| status.eq_ignore_ascii_case("RELEASING"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::WatchProgress;

    fn season_release(anime_id: u32, season: u32, title: &str, episodes: u32) -> ReleaseEntry {
        ReleaseEntry {
            anihub_id: Some(anime_id),
            anilist_id: Some(anime_id + 10_000),
            title: title.to_string(),
            anime_type: "TV".to_string(),
            year: Some(2014 + season),
            poster_url: None,
            episodes_count: Some(episodes),
            available_episodes: Some(episodes),
            airing_status: None,
            next_airing_episode: None,
            next_airing_at: None,
            description: None,
            rating: None,
            genres: None,
            dubbing_studios: None,
            conceptual_season: Some(season),
            part: Some(1),
            classification: ReleaseClassification::MainlineSeason,
            availability: ReleaseAvailability::Available,
        }
    }

    fn two_season_catalog() -> FranchiseCatalog {
        FranchiseCatalog {
            anchor_anilist_id: Some(10_001),
            canonical_title: "Клас убивць".to_string(),
            canonical_poster_url: None,
            unresolved_anilist_ids: Vec::new(),
            releases: vec![
                season_release(1, 1, "Клас убивць", 22),
                season_release(2, 2, "Клас убивць - 2 сезон", 25),
            ],
        }
    }

    fn progress(anime_id: u32, season: u32, episode: u32, watched: bool) -> WatchProgress {
        WatchProgress {
            anime_id,
            anime_title: "Клас убивць".to_string(),
            season,
            episode,
            studio_name: "FanWoxUA".to_string(),
            timestamp: 25.0,
            duration: 1_380.0,
            watched,
            updated_at: i64::from(episode),
        }
    }

    #[test]
    fn sibling_seasons_are_materialized_from_one_release_progress() {
        let mut history = AppHistory::default();
        history
            .progress
            .insert("2:2:20:FanWoxUA".to_string(), progress(2, 2, 20, false));

        let updates = library_catalog_updates(&history, &[two_season_catalog()]);

        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].anime_id, 1);
        assert_eq!(updates[0].status, AnimeStatus::NotAdded);
        assert_eq!(
            updates[0].release.as_ref().map(|release| release.season),
            Some(1)
        );
        assert_eq!(updates[1].anime_id, 2);
        assert_eq!(updates[1].status, AnimeStatus::Watching);
        assert_eq!(
            updates[1].release.as_ref().map(|release| release.season),
            Some(2)
        );
    }

    #[test]
    fn new_seasons_movies_and_specials_remain_visible_but_unplanned() {
        let mut history = AppHistory::default();
        let catalog = two_season_catalog();
        history.library.insert(
            1,
            crate::storage::history::AnimeLibraryRecord {
                title: "Клас убивць".to_string(),
                status: AnimeStatus::Completed,
                updated_at: 10,
                release: Some(library_metadata_for_release(&catalog, &catalog.releases[0])),
            },
        );
        let mut catalog = catalog;
        let mut movie = season_release(3, 2, "Клас убивць: Фільм", 1);
        movie.classification = ReleaseClassification::MainlineMovie;
        catalog.releases.push(movie);
        let mut special = season_release(4, 2, "Клас убивць: OVA", 1);
        special.classification = ReleaseClassification::MainlineSpecial;
        catalog.releases.push(special);

        let updates = library_catalog_updates(&history, &[catalog]);

        assert_eq!(updates.len(), 3);
        assert!(
            updates
                .iter()
                .all(|update| update.status == AnimeStatus::NotAdded)
        );
        assert_eq!(
            updates
                .iter()
                .find(|update| update.anime_id == 3)
                .and_then(|update| update.release.as_ref())
                .map(|release| release.kind),
            Some(LibraryReleaseKind::Movie)
        );
        assert_eq!(
            updates
                .iter()
                .find(|update| update.anime_id == 4)
                .and_then(|update| update.release.as_ref())
                .map(|release| release.kind),
            Some(LibraryReleaseKind::Special)
        );
    }

    #[test]
    fn fully_watched_ongoing_release_stays_watching() {
        let mut history = AppHistory::default();
        for episode in 1..=3 {
            history
                .progress
                .insert(format!("7:1:{episode}:Dub"), progress(7, 1, episode, true));
        }
        let metadata = LibraryReleaseMetadata {
            title: "Онгоінг".to_string(),
            kind: LibraryReleaseKind::Season,
            season: 1,
            part: Some(1),
            episodes_count: Some(3),
            first_episode: Some(1),
            airing_status: Some("RELEASING".to_string()),
            next_airing_episode: Some(4),
            next_airing_at: Some(2_000_000_000),
        };

        assert_eq!(
            inferred_release_status(&history, 7, &metadata),
            AnimeStatus::Watching
        );
        let mut finished = metadata;
        finished.airing_status = Some("FINISHED".to_string());
        finished.next_airing_episode = None;
        finished.next_airing_at = None;
        assert_eq!(
            inferred_release_status(&history, 7, &finished),
            AnimeStatus::Completed
        );
    }

    #[test]
    fn refresh_reopens_a_completed_release_that_became_ongoing() {
        let mut history = AppHistory::default();
        let mut catalog = two_season_catalog();
        catalog.releases.truncate(1);
        catalog.releases[0].airing_status = Some("RELEASING".to_string());
        catalog.releases[0].next_airing_episode = Some(23);
        history.library.insert(
            1,
            crate::storage::history::AnimeLibraryRecord {
                title: "Клас убивць".to_string(),
                status: AnimeStatus::Completed,
                updated_at: 10,
                release: Some(library_metadata_for_release(&catalog, &catalog.releases[0])),
            },
        );

        let updates = library_catalog_updates(&history, &[catalog]);

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].status, AnimeStatus::Watching);
    }

    #[test]
    fn cached_catalog_does_not_resurrect_an_explicitly_removed_franchise() {
        let mut history = AppHistory::default();
        history
            .progress
            .insert("2:2:20:FanWoxUA".to_string(), progress(2, 2, 20, false));
        history.library.insert(
            2,
            crate::storage::history::AnimeLibraryRecord {
                title: "Клас убивць".to_string(),
                status: AnimeStatus::NotAdded,
                updated_at: 11,
                release: None,
            },
        );

        assert!(library_catalog_updates(&history, &[two_season_catalog()]).is_empty());
    }
}
