//! Pure projection of persisted history into Library rows.

use super::{LibraryAnimeEntry, LibraryFilter, LibrarySeasonEntry, LibrarySort};
use crate::api::AnimeDetails;
use crate::storage::{AnimeStatus, AppHistory, LibraryReleaseKind, WatchProgress};
use std::collections::{HashMap, HashSet};

pub(super) fn build_library_items(history: &AppHistory) -> Vec<LibraryAnimeEntry> {
    let mut title_by_id = HashMap::<u32, String>::new();
    for progress in history.progress.values() {
        title_by_id
            .entry(progress.anime_id)
            .or_insert_with(|| progress.anime_title.clone());
    }
    for (&anime_id, record) in &history.library {
        title_by_id.insert(anime_id, record.title.clone());
    }

    let mut ids_by_title = HashMap::<String, Vec<u32>>::new();
    for (anime_id, title) in title_by_id {
        ids_by_title.entry(title).or_default().push(anime_id);
    }

    let mut items = Vec::new();
    for (anime_title, mut anime_ids) in ids_by_title {
        anime_ids.sort_unstable();
        anime_ids.dedup();
        let franchise_active = anime_ids.iter().any(|anime_id| {
            history.library.get(anime_id).map_or_else(
                || {
                    history
                        .progress
                        .values()
                        .any(|progress| progress.anime_id == *anime_id)
                },
                |record| record.status != AnimeStatus::NotAdded,
            )
        });
        if !franchise_active {
            continue;
        }

        let explicit_statuses = anime_ids
            .iter()
            .filter_map(|anime_id| history.library.get(anime_id))
            .filter(|record| record.status != AnimeStatus::NotAdded)
            .collect::<Vec<_>>();
        let status = if !explicit_statuses.is_empty()
            && explicit_statuses
                .iter()
                .all(|record| record.status == AnimeStatus::Completed)
        {
            AnimeStatus::Completed
        } else if explicit_statuses.iter().any(|record| {
            matches!(
                record.status,
                AnimeStatus::Watching | AnimeStatus::Completed
            )
        }) {
            AnimeStatus::Watching
        } else {
            explicit_statuses
                .into_iter()
                .max_by_key(|record| record.updated_at)
                .map_or(AnimeStatus::Watching, |record| record.status)
        };

        let mut seasons = anime_ids
            .iter()
            .enumerate()
            .map(|(release_index, &anime_id)| {
                let record = history.library.get(&anime_id);
                let metadata = record.and_then(|record| record.release.clone());
                let mut episodes = history
                    .progress
                    .values()
                    .filter(|progress| progress.anime_id == anime_id)
                    .cloned()
                    .collect::<Vec<_>>();
                episodes.sort_by_key(|progress| (progress.episode, progress.updated_at));
                let season = metadata
                    .as_ref()
                    .map(|release| release.season)
                    .or_else(|| episodes.first().map(|progress| progress.season))
                    .unwrap_or(release_index as u32 + 1);
                LibrarySeasonEntry {
                    anime_id,
                    season,
                    part: metadata.as_ref().and_then(|release| release.part),
                    title: metadata
                        .as_ref()
                        .map(|release| release.title.clone())
                        .unwrap_or_else(|| format!("Сезон {season}")),
                    kind: metadata
                        .as_ref()
                        .map_or(LibraryReleaseKind::Season, |release| release.kind),
                    episodes_count: metadata
                        .as_ref()
                        .and_then(|release| release.episodes_count)
                        .or_else(|| episodes.iter().map(|progress| progress.episode).max()),
                    first_episode: metadata
                        .as_ref()
                        .and_then(|release| release.first_episode)
                        .or_else(|| episodes.iter().map(|progress| progress.episode).min()),
                    airing_status: metadata
                        .as_ref()
                        .and_then(|release| release.airing_status.clone()),
                    next_airing_episode: metadata
                        .as_ref()
                        .and_then(|release| release.next_airing_episode),
                    next_airing_at: metadata.as_ref().and_then(|release| release.next_airing_at),
                    status: record.map_or(AnimeStatus::Watching, |record| record.status),
                    episodes,
                }
            })
            .collect::<Vec<_>>();
        seasons.sort_by_key(|release| {
            (
                match release.kind {
                    LibraryReleaseKind::Season => 0,
                    LibraryReleaseKind::Movie => 1,
                    LibraryReleaseKind::Special => 2,
                    LibraryReleaseKind::Extra => 3,
                },
                release.season,
                release.part.unwrap_or(1),
                release.anime_id,
            )
        });

        let latest_progress = history
            .progress
            .values()
            .filter(|progress| anime_ids.contains(&progress.anime_id))
            .max_by_key(|progress| progress.updated_at)
            .cloned()
            .unwrap_or_else(|| {
                let latest_record = anime_ids
                    .iter()
                    .filter_map(|anime_id| {
                        history
                            .library
                            .get(anime_id)
                            .map(|record| (*anime_id, record))
                    })
                    .max_by_key(|(_, record)| record.updated_at);
                let (anime_id, updated_at) = latest_record
                    .map(|(anime_id, record)| (anime_id, record.updated_at))
                    .unwrap_or((anime_ids[0], 0));
                let season = seasons
                    .iter()
                    .find(|release| release.anime_id == anime_id)
                    .map_or(1, |release| release.season);
                WatchProgress {
                    anime_id,
                    anime_title: anime_title.clone(),
                    season,
                    episode: 1,
                    studio_name: String::new(),
                    timestamp: 0.0,
                    duration: 0.0,
                    watched: false,
                    updated_at,
                }
            });

        items.push(LibraryAnimeEntry {
            anime_ids,
            anime_title,
            latest_progress,
            seasons,
            status,
        });
    }
    items.sort_by(|a, b| {
        b.latest_progress
            .updated_at
            .cmp(&a.latest_progress.updated_at)
    });
    items
}

pub(super) fn sort_library_items(
    items: &mut [LibraryAnimeEntry],
    sort: LibrarySort,
    reversed: bool,
    details_cache: &moka::sync::Cache<u32, AnimeDetails>,
    watched_index: &HashSet<(u32, u32, u32)>,
) {
    items.sort_by(|a, b| {
        let ordering = match sort {
            LibrarySort::Recent => b
                .latest_progress
                .updated_at
                .cmp(&a.latest_progress.updated_at),
            LibrarySort::Title => a
                .anime_title
                .to_lowercase()
                .cmp(&b.anime_title.to_lowercase()),
            LibrarySort::Year => compare_optional_desc(
                library_first_year(a, details_cache),
                library_first_year(b, details_cache),
                Ord::cmp,
            ),
            LibrarySort::Rating => compare_optional_desc(
                library_first_rating(a, details_cache),
                library_first_rating(b, details_cache),
                f32::total_cmp,
            ),
            LibrarySort::Progress => match (
                library_progress_ratio(a, watched_index),
                library_progress_ratio(b, watched_index),
            ) {
                (Some((a_watched, a_total)), Some((b_watched, b_total))) => {
                    (b_watched * a_total).cmp(&(a_watched * b_total))
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
        };
        let ordering = ordering
            .then_with(|| {
                b.latest_progress
                    .updated_at
                    .cmp(&a.latest_progress.updated_at)
            })
            .then_with(|| a.anime_title.cmp(&b.anime_title));
        if reversed {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

fn compare_optional_desc<T>(
    left: Option<T>,
    right: Option<T>,
    compare: impl FnOnce(&T, &T) -> std::cmp::Ordering,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => compare(&right, &left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn library_first_year(
    anime: &LibraryAnimeEntry,
    details_cache: &moka::sync::Cache<u32, AnimeDetails>,
) -> Option<u32> {
    anime
        .seasons
        .iter()
        .find_map(|release| details_cache.get(&release.anime_id)?.year)
}

fn library_first_rating(
    anime: &LibraryAnimeEntry,
    details_cache: &moka::sync::Cache<u32, AnimeDetails>,
) -> Option<f32> {
    anime
        .seasons
        .iter()
        .find_map(|release| details_cache.get(&release.anime_id)?.rating)
}

pub(super) fn library_progress_ratio(
    anime: &LibraryAnimeEntry,
    watched_index: &HashSet<(u32, u32, u32)>,
) -> Option<(u64, u64)> {
    let mut watched = 0u64;
    let mut total = 0u64;
    for release in &anime.seasons {
        let Some(release_total) = release.episodes_count else {
            continue;
        };
        total += u64::from(release_total);
        if release.status == AnimeStatus::Completed {
            watched += u64::from(release_total);
            continue;
        }
        let first = release.first_episode.unwrap_or(1);
        let end = first.saturating_add(release_total);
        watched += watched_index
            .iter()
            .filter(|(anime_id, season, episode)| {
                *anime_id == release.anime_id
                    && *season == release.season
                    && *episode >= first
                    && *episode < end
            })
            .count()
            .min(release_total as usize) as u64;
    }
    (total > 0).then_some((watched, total))
}

pub(super) fn library_item_matches(
    anime: &LibraryAnimeEntry,
    filter: LibraryFilter,
    query: &str,
) -> bool {
    let status_matches = match filter {
        LibraryFilter::All => true,
        LibraryFilter::Watching => anime.status == AnimeStatus::Watching,
        LibraryFilter::Planned => anime.status == AnimeStatus::Planned,
        LibraryFilter::Completed => anime.status == AnimeStatus::Completed,
        LibraryFilter::OnHold => anime.status == AnimeStatus::OnHold,
        LibraryFilter::Dropped => anime.status == AnimeStatus::Dropped,
    };
    let query = query.trim().to_lowercase();
    status_matches && (query.is_empty() || anime.anime_title.to_lowercase().contains(&query))
}

pub(super) fn anime_is_fully_watched(anime: &LibraryAnimeEntry) -> bool {
    !anime.seasons.is_empty()
        && anime
            .seasons
            .iter()
            .all(|season| season.episodes.iter().all(|episode| episode.watched))
}
