#![allow(dead_code, clippy::items_after_test_module)]

use super::models::{AnimeItem, AshdiStudio, EpisodeSourcesResponse};
use std::collections::{HashMap, HashSet};

/// A source entry that owns the AniHub anime id it came from.
///
/// The legacy UI currently keeps that id in a parallel vector.  New callers
/// can carry this value with the source itself and avoid index coupling.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedSource {
    pub anime_id: u32,
    pub source: AshdiStudio,
}

/// Descriptive alias for callers that work specifically with Ashdi sources.
pub type OwnedAshdiStudio = OwnedSource;

#[derive(Debug, Clone, PartialEq)]
pub struct CombinedEpisodeSources {
    pub ashdi: Vec<OwnedSource>,
}

impl CombinedEpisodeSources {
    pub fn is_empty(&self) -> bool {
        self.ashdi.is_empty()
    }

    /// Convert to the old response plus parallel owner ids for compatibility
    /// with the pre-redesign UI.
    pub fn into_legacy(self) -> (EpisodeSourcesResponse, Vec<u32>) {
        let anime_ids = self.ashdi.iter().map(|entry| entry.anime_id).collect();
        let ashdi = self.ashdi.into_iter().map(|entry| entry.source).collect();
        (
            EpisodeSourcesResponse {
                ashdi,
                moonanime: Vec::new(),
            },
            anime_ids,
        )
    }
}

/// Видаляє дублікати по `id`.
pub fn deduplicate_anime(items: Vec<AnimeItem>) -> Vec<AnimeItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.id))
        .collect()
}

/// Групує індекси в `items` за франшизою через prefix-matching назв.
/// Кожна група відсортована за роком (від старого до нового).
pub fn group_into_franchises(items: &[AnimeItem]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();

    'outer: for (i, item) in items.iter().enumerate() {
        for group in &mut groups {
            if same_franchise(&items[group[0]].title_ukrainian, &item.title_ukrainian) {
                group.push(i);
                continue 'outer;
            }
        }
        groups.push(vec![i]);
    }

    for group in &mut groups {
        group.sort_by_key(|&i| items[i].year.unwrap_or(0));
    }

    groups
}

/// Індекс найкращого представника групи для завантаження джерел серій.
/// Перевага: найновіший TV-запис; fallback: останній за роком.
pub fn representative_idx(items: &[AnimeItem], group: &[usize]) -> usize {
    for &idx in group.iter().rev() {
        let t = items[idx].anime_type.to_lowercase();
        if !t.contains("ova") && !t.contains("спец") && !t.contains("special") {
            return idx;
        }
    }
    *group.last().unwrap()
}

/// Базова назва франшизи — найкоротший рядок у групі.
pub fn franchise_display_name<'a>(items: &'a [AnimeItem], group: &[usize]) -> &'a str {
    group
        .iter()
        .map(|&i| items[i].title_ukrainian.as_str())
        .min_by_key(|s| s.len())
        .unwrap_or("")
}

/// Combine already-fetched franchise source responses in a deterministic
/// manner.  `franchise_order` is the canonical order (normally AniList order
/// or the UI's sorted TV-member order); `results` may arrive in any order.
///
/// The function is deliberately pure: it performs no I/O, no cache mutation,
/// and no task scheduling.  Duplicate `(season, studio)` entries keep the
/// response with more episodes, with stable tie breakers for equal lengths.
pub fn combine_franchise_sources(
    franchise_order: &[u32],
    results: &[(u32, EpisodeSourcesResponse)],
) -> CombinedEpisodeSources {
    let mut order = Vec::new();
    let mut order_position = HashMap::new();
    for &anime_id in franchise_order {
        if order_position.contains_key(&anime_id) {
            continue;
        }
        order_position.insert(anime_id, order.len());
        order.push(anime_id);
    }

    // Results are copied into canonical member order, making completion order
    // irrelevant.  An unknown id is still accepted after known members, in a
    // stable numeric order, so callers do not lose a response accidentally.
    let mut by_id = HashMap::<u32, EpisodeSourcesResponse>::new();
    for (anime_id, response) in results {
        by_id
            .entry(*anime_id)
            .and_modify(|existing| {
                if response_signature(response) < response_signature(existing) {
                    *existing = response.clone();
                }
            })
            .or_insert_with(|| response.clone());
    }
    let mut unknown_ids = by_id
        .keys()
        .copied()
        .filter(|anime_id| !order_position.contains_key(anime_id))
        .collect::<Vec<_>>();
    unknown_ids.sort_unstable();
    order.extend(unknown_ids);

    let mut members = Vec::<(u32, Vec<AshdiStudio>)>::new();
    for anime_id in order {
        let Some(response) = by_id.remove(&anime_id) else {
            continue;
        };
        let mut studios = response.ashdi;
        studios.sort_by(studio_cmp);
        deduplicate_member_studios(&mut studios);
        if !studios.is_empty() {
            members.push((anime_id, studios));
        }
    }

    if members.is_empty() {
        return CombinedEpisodeSources { ashdi: Vec::new() };
    }

    let mut best_by_season_studio = Vec::<(u32, usize, AshdiStudio)>::new();
    for (member_position, (anime_id, studios)) in members.iter().enumerate() {
        for studio in studios {
            let _ = anime_id;
            if let Some(position) = best_by_season_studio.iter().position(|(_, _, existing)| {
                existing.season_number == studio.season_number
                    && existing.studio_name == studio.studio_name
            }) {
                let (_, existing_member_position, existing) = &best_by_season_studio[position];
                if is_better_studio(studio, member_position, existing, *existing_member_position) {
                    best_by_season_studio[position] =
                        (studio.season_number, member_position, studio.clone());
                }
            } else {
                best_by_season_studio.push((studio.season_number, member_position, studio.clone()));
            }
        }
    }

    best_by_season_studio.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.2.studio_name.cmp(&right.2.studio_name))
            .then_with(|| left.2.id.cmp(&right.2.id))
            .then_with(|| left.1.cmp(&right.1))
    });

    let mut season_numbers = best_by_season_studio
        .iter()
        .map(|(season, _, _)| *season)
        .collect::<Vec<_>>();
    season_numbers.sort_unstable();
    season_numbers.dedup();

    let active_members = members.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let mut combined = Vec::new();

    // Keep the existing franchise alignment behavior: when there are enough
    // source-bearing members, each member is given one successive season when
    // possible.  Otherwise, the best source per season is retained and its
    // owner is mapped to the nearest active member deterministically.
    if active_members.len() >= season_numbers.len() {
        let mut claimed_urls = HashSet::new();
        let mut normalized_season = 1u32;
        for (member_position, (anime_id, studios)) in members.iter().enumerate() {
            let chosen_original_season = season_numbers
                .get(member_position)
                .copied()
                .or_else(|| {
                    studios
                        .iter()
                        .find(|studio| studio.season_number == normalized_season)
                        .map(|studio| studio.season_number)
                })
                .or_else(|| {
                    studios
                        .iter()
                        .map(|studio| studio.season_number)
                        .find(|season| {
                            studios
                                .iter()
                                .filter(|studio| studio.season_number == *season)
                                .any(|studio| {
                                    studio
                                        .episodes
                                        .first()
                                        .map(|episode| !claimed_urls.contains(&episode.url))
                                        .unwrap_or(false)
                                })
                        })
                });
            let Some(chosen_original_season) = chosen_original_season else {
                continue;
            };

            for studio in studios
                .iter()
                .filter(|studio| studio.season_number == chosen_original_season)
            {
                if let Some(episode) = studio.episodes.first() {
                    claimed_urls.insert(episode.url.clone());
                }
                let mut source = studio.clone();
                source.season_number = normalized_season;
                combined.push(OwnedSource {
                    anime_id: *anime_id,
                    source,
                });
            }
            normalized_season += 1;
        }
    } else {
        let member_offset = active_members.len().saturating_sub(season_numbers.len());
        for (original_season, member_position, studio) in best_by_season_studio {
            let season_position = season_numbers
                .iter()
                .position(|season| *season == original_season)
                .unwrap_or(0);
            let owner_id = active_members
                .get(season_position + member_offset)
                .copied()
                .unwrap_or_else(|| active_members.last().copied().unwrap_or(0));
            let mut source = studio;
            source.season_number = season_position as u32 + 1;
            let _ = member_position;
            combined.push(OwnedSource {
                anime_id: owner_id,
                source,
            });
        }
    }

    combined.sort_by(|left, right| {
        left.source
            .season_number
            .cmp(&right.source.season_number)
            .then_with(|| left.source.studio_name.cmp(&right.source.studio_name))
            .then_with(|| left.anime_id.cmp(&right.anime_id))
            .then_with(|| left.source.id.cmp(&right.source.id))
    });
    CombinedEpisodeSources { ashdi: combined }
}

/// Compatibility wrapper for code that still expects the old response and
/// parallel owner-id vector.
pub fn combine_franchise_sources_legacy(
    franchise_order: &[u32],
    results: &[(u32, EpisodeSourcesResponse)],
) -> Option<(EpisodeSourcesResponse, Vec<u32>)> {
    let combined = combine_franchise_sources(franchise_order, results);
    (!combined.is_empty()).then(|| combined.into_legacy())
}

fn studio_cmp(left: &AshdiStudio, right: &AshdiStudio) -> std::cmp::Ordering {
    left.season_number
        .cmp(&right.season_number)
        .then_with(|| left.studio_name.cmp(&right.studio_name))
        .then_with(|| left.id.cmp(&right.id))
        .then_with(|| right.episodes.len().cmp(&left.episodes.len()))
}

fn response_signature(response: &EpisodeSourcesResponse) -> String {
    serde_json::to_string(response).unwrap_or_default()
}

fn deduplicate_member_studios(studios: &mut Vec<AshdiStudio>) {
    let mut deduplicated = Vec::with_capacity(studios.len());
    for studio in studios.drain(..) {
        if let Some(position) = deduplicated.iter().position(|existing: &AshdiStudio| {
            existing.season_number == studio.season_number
                && existing.studio_name == studio.studio_name
        }) {
            let existing = &deduplicated[position];
            if is_better_studio(&studio, 0, existing, 0) {
                deduplicated[position] = studio;
            }
        } else {
            deduplicated.push(studio);
        }
    }
    deduplicated.sort_by(studio_cmp);
    *studios = deduplicated;
}

fn is_better_studio(
    candidate: &AshdiStudio,
    candidate_member_position: usize,
    existing: &AshdiStudio,
    existing_member_position: usize,
) -> bool {
    candidate
        .episodes
        .len()
        .cmp(&existing.episodes.len())
        .then_with(|| candidate.episodes_count.cmp(&existing.episodes_count))
        .then_with(|| existing.id.cmp(&candidate.id))
        .then_with(|| existing_member_position.cmp(&candidate_member_position))
        .then_with(|| {
            response_signature(&EpisodeSourcesResponse {
                ashdi: vec![candidate.clone()],
                moonanime: Vec::new(),
            })
            .cmp(&response_signature(&EpisodeSourcesResponse {
                ashdi: vec![existing.clone()],
                moonanime: Vec::new(),
            }))
        })
        == std::cmp::Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio(id: u32, season: u32, name: &str, url: &str) -> AshdiStudio {
        AshdiStudio {
            id,
            studio_name: name.to_string(),
            season_number: season,
            episodes: vec![super::super::models::AshdiEpisode {
                episode_number: 1,
                display_episode_number: None,
                title: "Episode".to_string(),
                url: url.to_string(),
                ashdi_episode_id: String::new(),
            }],
            episodes_count: 1,
        }
    }

    #[test]
    fn combine_is_independent_of_result_completion_order_and_keeps_owner() {
        let one = EpisodeSourcesResponse {
            ashdi: vec![studio(10, 1, "A", "one")],
            moonanime: Vec::new(),
        };
        let two = EpisodeSourcesResponse {
            ashdi: vec![studio(20, 2, "B", "two")],
            moonanime: Vec::new(),
        };
        let first = combine_franchise_sources(&[1, 2], &[(1, one.clone()), (2, two.clone())]);
        let second = combine_franchise_sources(&[1, 2], &[(2, two), (1, one)]);
        assert_eq!(first, second);
        assert_eq!(first.ashdi[0].anime_id, 1);
        assert_eq!(first.ashdi[1].anime_id, 2);
        assert_eq!(first.ashdi[0].source.season_number, 1);
        assert_eq!(first.ashdi[1].source.season_number, 2);
    }
}

/// Базовий префікс назви до першого ':' або '?', без числових суфіксів.
/// "Реінкарнація безробітного 2: ..." → "Реінкарнація безробітного"
fn franchise_base(s: &str) -> &str {
    let end = s.find(':').or_else(|| s.find('?')).unwrap_or(s.len());
    s[..end].trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ')
}

fn same_franchise(a: &str, b: &str) -> bool {
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if let Some(rest) = longer.strip_prefix(shorter) {
        return rest.is_empty()
            || rest.starts_with(' ')
            || rest.starts_with(':')
            || rest.starts_with('?');
    }
    // Кейс "Назва: ..." ↔ "Назва 2: ..." або "Назва II: ..." — порівнюємо базу до ':'
    let ba = franchise_base(a);
    let bb = franchise_base(b);
    ba.len() >= 15 && ba == bb
}
