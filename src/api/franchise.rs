//! Release-level franchise catalog construction.
//!
//! AniHub search results tell us which releases are locally available while
//! AniList relationships provide the graph that connects sequels and extras.
//! This module deliberately performs no I/O: callers fetch one AniList batch
//! and then build deterministic catalogs from the two data sets.

use super::models::{AnimeItem, DubbingStudio};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

static SEASON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:season|сезон)\s*[-:№]?\s*(\d+)").expect("valid season regex")
});
static ORDINAL_SEASON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d+)(?:st|nd|rd|th)\s+season\b").expect("valid ordinal season regex")
});
static PART_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:part|частина)\s*[-:№]?\s*(\d+)").expect("valid part regex")
});
static NUMBERED_PART_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d+)\s*(?:частина)\b").expect("valid numbered part regex")
});
static ROMAN_SEASON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(II|III|IV|V|VI|VII|VIII|IX|X)\b").expect("valid roman season regex")
});

/// AniList title fields returned by the batch query.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AniListTitle {
    #[serde(default)]
    pub romaji: Option<String>,
    #[serde(default)]
    pub english: Option<String>,
    #[serde(default)]
    pub native: Option<String>,
}

impl AniListTitle {
    pub fn display_name(&self) -> Option<&str> {
        self.english
            .as_deref()
            .or(self.romaji.as_deref())
            .or(self.native.as_deref())
    }

    fn candidates(&self) -> impl Iterator<Item = &str> {
        self.english
            .iter()
            .chain(self.romaji.iter())
            .chain(self.native.iter())
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AniListCoverImage {
    #[serde(default)]
    pub large: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AniListNextAiringEpisode {
    pub episode: u32,
    #[serde(default, rename = "airingAt")]
    pub airing_at: Option<i64>,
}

/// A relation node does not recursively include its own relations.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AniListMediaNode {
    pub id: u32,
    #[serde(default, rename = "type")]
    pub media_type: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub episodes: Option<u32>,
    #[serde(default, rename = "seasonYear")]
    pub season_year: Option<u32>,
    #[serde(default, rename = "nextAiringEpisode")]
    pub next_airing_episode: Option<AniListNextAiringEpisode>,
    #[serde(default)]
    pub title: AniListTitle,
    #[serde(default, rename = "coverImage")]
    pub cover_image: AniListCoverImage,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AniListRelationEdge {
    #[serde(rename = "relationType")]
    pub relation_type: String,
    pub node: AniListMediaNode,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AniListRelations {
    #[serde(default)]
    pub edges: Vec<AniListRelationEdge>,
}

/// One full media record returned by [`ApiClient::get_anilist_media_batch`].
///
/// [`ApiClient::get_anilist_media_batch`]: super::client::ApiClient::get_anilist_media_batch
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AniListMedia {
    pub id: u32,
    #[serde(default, rename = "type")]
    pub media_type: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub episodes: Option<u32>,
    #[serde(default, rename = "seasonYear")]
    pub season_year: Option<u32>,
    #[serde(default, rename = "nextAiringEpisode")]
    pub next_airing_episode: Option<AniListNextAiringEpisode>,
    #[serde(default)]
    pub title: AniListTitle,
    #[serde(default, rename = "coverImage")]
    pub cover_image: AniListCoverImage,
    #[serde(default)]
    pub relations: AniListRelations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReleaseClassification {
    MainlineSeason,
    MainlineMovie,
    MainlineSpecial,
    Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReleaseAvailability {
    Available,
    Unavailable,
}

/// A single AniList release projected into the local AniHub catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseEntry {
    pub anihub_id: Option<u32>,
    pub anilist_id: Option<u32>,
    pub title: String,
    pub anime_type: String,
    pub year: Option<u32>,
    pub poster_url: Option<String>,
    pub episodes_count: Option<u32>,
    /// Episodes that have actually aired. AniHub may expose future placeholder
    /// VOD rows for an ongoing release, so source lists are capped to this.
    pub available_episodes: Option<u32>,
    pub description: Option<String>,
    pub rating: Option<f32>,
    pub genres: Option<Vec<String>>,
    pub dubbing_studios: Option<Vec<DubbingStudio>>,
    pub conceptual_season: Option<u32>,
    pub part: Option<u32>,
    pub classification: ReleaseClassification,
    pub availability: ReleaseAvailability,
}

/// One connected AniList component containing at least one AniHub release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FranchiseCatalog {
    pub anchor_anilist_id: Option<u32>,
    pub canonical_title: String,
    /// Poster of the first mainline release, not the currently selected cour.
    pub canonical_poster_url: Option<String>,
    /// Related mainline releases missing from the initial AniHub search.
    /// They stay hidden from the release list while callers verify their
    /// availability through AniHub's exact AniList-id lookup.
    #[serde(default)]
    pub unresolved_anilist_ids: Vec<u32>,
    pub releases: Vec<ReleaseEntry>,
}

#[derive(Debug, Clone)]
struct NodeRecord {
    id: u32,
    format: Option<String>,
    status: Option<String>,
    year: Option<u32>,
    episodes: Option<u32>,
    next_airing_episode: Option<u32>,
    title: AniListTitle,
    poster_url: Option<String>,
}

impl From<&AniListMedia> for NodeRecord {
    fn from(media: &AniListMedia) -> Self {
        Self {
            id: media.id,
            format: media.format.clone(),
            status: media.status.clone(),
            year: media.season_year,
            episodes: media.episodes,
            next_airing_episode: media
                .next_airing_episode
                .as_ref()
                .map(|airing| airing.episode),
            title: media.title.clone(),
            poster_url: media.cover_image.large.clone(),
        }
    }
}

impl From<&AniListMediaNode> for NodeRecord {
    fn from(media: &AniListMediaNode) -> Self {
        Self {
            id: media.id,
            format: media.format.clone(),
            status: media.status.clone(),
            year: media.season_year,
            episodes: media.episodes,
            next_airing_episode: media
                .next_airing_episode
                .as_ref()
                .map(|airing| airing.episode),
            title: media.title.clone(),
            poster_url: media.cover_image.large.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainlineDirection {
    CurrentBeforeNode,
    NodeBeforeCurrent,
}

/// Build every connected franchise represented by the supplied AniHub items.
/// Prefix similarity is intentionally ignored; only AniList edges connect two
/// releases. Results and releases are stable regardless of input order.
pub fn build_franchise_catalogs(
    anihub_releases: &[AnimeItem],
    anilist_media: &[AniListMedia],
) -> Vec<FranchiseCatalog> {
    let mut available_by_anilist = HashMap::<u32, AnimeItem>::new();
    let mut without_anilist = Vec::new();
    for release in anihub_releases {
        if let Some(anilist_id) = release.anilist_id {
            available_by_anilist
                .entry(anilist_id)
                .and_modify(|existing| {
                    if release.id < existing.id {
                        *existing = release.clone();
                    }
                })
                .or_insert_with(|| release.clone());
        } else {
            without_anilist.push(release.clone());
        }
    }

    let mut nodes = BTreeMap::<u32, NodeRecord>::new();
    // Only sequel/prequel edges are allowed to form franchise components.
    // Side stories and anthology specials are leaf attachments; traversing
    // through them can merge unrelated franchises (for example several Jump
    // Festa shorts that share one omnibus node).
    let mut mainline_adjacency = HashMap::<u32, BTreeSet<u32>>::new();
    let mut mainline_edges = BTreeSet::<(u32, u32)>::new();
    let mut extra_edges = BTreeSet::<(u32, u32)>::new();

    for media in anilist_media {
        if !is_anime(media.media_type.as_deref()) {
            continue;
        }
        nodes.insert(media.id, NodeRecord::from(media));
        mainline_adjacency.entry(media.id).or_default();
        for edge in &media.relations.edges {
            if !is_anime(edge.node.media_type.as_deref()) {
                continue;
            }
            nodes
                .entry(edge.node.id)
                .or_insert_with(|| NodeRecord::from(&edge.node));
            if let Some(direction) = mainline_direction(&edge.relation_type) {
                mainline_adjacency
                    .entry(media.id)
                    .or_default()
                    .insert(edge.node.id);
                mainline_adjacency
                    .entry(edge.node.id)
                    .or_default()
                    .insert(media.id);
                let directed = match direction {
                    MainlineDirection::CurrentBeforeNode => (media.id, edge.node.id),
                    MainlineDirection::NodeBeforeCurrent => (edge.node.id, media.id),
                };
                mainline_edges.insert(directed);
            } else {
                mainline_adjacency.entry(edge.node.id).or_default();
                extra_edges.insert((media.id, edge.node.id));
            }
        }
    }

    for (&anilist_id, release) in &available_by_anilist {
        nodes.entry(anilist_id).or_insert_with(|| NodeRecord {
            id: anilist_id,
            format: Some(release.anime_type.clone()),
            status: Some(release.status.clone()),
            year: release.year,
            episodes: release.episodes_count,
            next_airing_episode: None,
            title: AniListTitle {
                english: release.title_english.clone(),
                romaji: release.title_original.clone(),
                native: None,
            },
            poster_url: release.poster_url.clone(),
        });
        mainline_adjacency.entry(anilist_id).or_default();
    }

    let mut components = Vec::<BTreeSet<u32>>::new();
    let mut component_by_node = HashMap::<u32, usize>::new();
    let mut visited = HashSet::new();
    for &start in nodes.keys() {
        if visited.contains(&start) {
            continue;
        }
        let component = connected_component(start, &mainline_adjacency, &mut visited);
        let index = components.len();
        for id in &component {
            component_by_node.insert(*id, index);
        }
        components.push(component);
    }

    let component_has_available_mainline = components
        .iter()
        .map(|component| {
            component.iter().any(|id| {
                available_by_anilist.get(id).is_some_and(|local| {
                    !is_extra_attachment(
                        nodes.get(id).expect("available component node exists"),
                        Some(local),
                    )
                })
            })
        })
        .collect::<Vec<_>>();
    let mut attachments = vec![BTreeSet::<u32>::new(); components.len()];
    let mut claimed_extras = HashSet::<u32>::new();
    for (left, right) in extra_edges {
        let Some(&left_component) = component_by_node.get(&left) else {
            continue;
        };
        let Some(&right_component) = component_by_node.get(&right) else {
            continue;
        };
        if left_component == right_component {
            continue;
        }
        let left_is_extra = is_extra_attachment(
            nodes.get(&left).expect("relation node exists"),
            available_by_anilist.get(&left),
        );
        let right_is_extra = is_extra_attachment(
            nodes.get(&right).expect("relation node exists"),
            available_by_anilist.get(&right),
        );
        let attachment = match (left_is_extra, right_is_extra) {
            (false, true) if component_has_available_mainline[left_component] => {
                Some((left_component, right))
            }
            (true, false) if component_has_available_mainline[right_component] => {
                Some((right_component, left))
            }
            _ => None,
        };
        let Some((main_component, extra_id)) = attachment else {
            continue;
        };
        // Extras are intentionally local-only. Unavailable relation nodes are
        // noisy metadata, not playable Ukrainian releases.
        if available_by_anilist.contains_key(&extra_id) {
            attachments[main_component].insert(extra_id);
            claimed_extras.insert(extra_id);
        }
    }

    let mut catalogs = Vec::new();
    for (index, mut component) in components.into_iter().enumerate() {
        if !component
            .iter()
            .any(|id| available_by_anilist.contains_key(id) && !claimed_extras.contains(id))
        {
            continue;
        }
        component.extend(attachments[index].iter().copied());
        catalogs.push(build_component_catalog(
            &component,
            &nodes,
            &available_by_anilist,
            &mainline_edges,
        ));
    }

    without_anilist.sort_by_key(|release| release.id);
    catalogs.extend(without_anilist.into_iter().map(single_release_catalog));
    catalogs.sort_by(|left, right| catalog_sort_key(left).cmp(&catalog_sort_key(right)));
    catalogs
}

fn is_extra_attachment(node: &NodeRecord, local: Option<&AnimeItem>) -> bool {
    let format = local
        .map(|release| release.anime_type.as_str())
        .or(node.format.as_deref())
        .unwrap_or_default();
    classification_for_format(format) != ReleaseClassification::MainlineSeason
}

/// Find the catalog containing a particular AniList release.
#[allow(dead_code)]
pub fn build_franchise_catalog(
    anihub_releases: &[AnimeItem],
    anilist_media: &[AniListMedia],
    anchor_anilist_id: u32,
) -> Option<FranchiseCatalog> {
    build_franchise_catalogs(anihub_releases, anilist_media)
        .into_iter()
        .find(|catalog| {
            catalog
                .releases
                .iter()
                .any(|release| release.anilist_id == Some(anchor_anilist_id))
        })
}

fn connected_component(
    start: u32,
    adjacency: &HashMap<u32, BTreeSet<u32>>,
    visited: &mut HashSet<u32>,
) -> BTreeSet<u32> {
    let mut component = BTreeSet::new();
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        component.insert(id);
        if let Some(neighbours) = adjacency.get(&id) {
            queue.extend(neighbours.iter().copied());
        }
    }
    component
}

fn build_component_catalog(
    component: &BTreeSet<u32>,
    nodes: &BTreeMap<u32, NodeRecord>,
    available: &HashMap<u32, AnimeItem>,
    all_mainline_edges: &BTreeSet<(u32, u32)>,
) -> FranchiseCatalog {
    let component_edges = all_mainline_edges
        .iter()
        .copied()
        .filter(|(from, to)| component.contains(from) && component.contains(to))
        .collect::<BTreeSet<_>>();
    let mut mainline = component_edges
        .iter()
        .flat_map(|(from, to)| [*from, *to])
        .collect::<BTreeSet<_>>();
    if mainline.is_empty() {
        let first_available = component
            .iter()
            .filter(|id| available.contains_key(id))
            .min_by_key(|id| node_sort_key(nodes.get(id).expect("component node")));
        if let Some(first_available) = first_available {
            mainline.insert(*first_available);
        }
    }

    let ordered_mainline = topological_mainline_order(&mainline, &component_edges, nodes);
    let mut conceptual = HashMap::<u32, (Option<u32>, Option<u32>)>::new();
    let mut last_tv_season = 0u32;
    for id in &ordered_mainline {
        let node = nodes.get(id).expect("mainline node exists");
        let release = available.get(id);
        let classification = classify(node, release, true);
        if classification != ReleaseClassification::MainlineSeason {
            conceptual.insert(*id, (None, None));
            continue;
        }
        let explicit_season = title_candidates(node, release).find_map(parse_season);
        let explicit_part = title_candidates(node, release).find_map(parse_part);
        let season = if let Some(season) = explicit_season {
            season
        } else if explicit_part.is_some_and(|part| part > 1) && last_tv_season > 0 {
            last_tv_season
        } else {
            last_tv_season.saturating_add(1).max(1)
        };
        last_tv_season = last_tv_season.max(season);
        conceptual.insert(*id, (Some(season), Some(explicit_part.unwrap_or(1))));
    }

    let mut order = ordered_mainline;
    let mut extras = component
        .iter()
        .copied()
        .filter(|id| !mainline.contains(id))
        .collect::<Vec<_>>();
    extras.sort_by_key(|id| node_sort_key(nodes.get(id).expect("extra node exists")));
    order.extend(extras);

    let unresolved_anilist_ids = order
        .iter()
        .copied()
        .filter(|id| !available.contains_key(id))
        .collect::<Vec<_>>();
    let releases = order
        .into_iter()
        // AniList-only nodes are useful while ordering and numbering the
        // franchise graph, but they are not actionable releases in AniHub.
        // Keep them out of the user-facing catalog entirely.
        .filter(|id| available.contains_key(id))
        .map(|id| {
            let node = nodes.get(&id).expect("component node exists");
            let local = available.get(&id);
            let is_mainline = mainline.contains(&id);
            let (conceptual_season, part) = conceptual.get(&id).copied().unwrap_or((None, None));
            ReleaseEntry {
                anihub_id: local.map(|release| release.id),
                anilist_id: Some(id),
                title: release_title(node, local),
                anime_type: local
                    .map(|release| release.anime_type.clone())
                    .or_else(|| node.format.clone())
                    .unwrap_or_default(),
                year: local.and_then(|release| release.year).or(node.year),
                poster_url: local
                    .and_then(|release| release.poster_url.clone())
                    .or_else(|| node.poster_url.clone()),
                episodes_count: local
                    .and_then(|release| release.episodes_count)
                    .or(node.episodes),
                available_episodes: aired_episode_count(node, local),
                description: local.and_then(|release| release.description.clone()),
                rating: local.and_then(|release| release.rating),
                genres: local.and_then(|release| release.genres.clone()),
                dubbing_studios: local.and_then(|release| release.dubbing_studios.clone()),
                conceptual_season,
                part,
                classification: classify(node, local, is_mainline),
                availability: if local.is_some() {
                    ReleaseAvailability::Available
                } else {
                    ReleaseAvailability::Unavailable
                },
            }
        })
        .collect::<Vec<_>>();

    let canonical = releases
        .iter()
        .find(|release| release.classification != ReleaseClassification::Extra)
        .or_else(|| releases.first())
        .expect("component with an available release is non-empty");
    FranchiseCatalog {
        anchor_anilist_id: canonical.anilist_id,
        canonical_title: canonical.title.clone(),
        canonical_poster_url: canonical.poster_url.clone(),
        unresolved_anilist_ids,
        releases,
    }
}

fn single_release_catalog(release: AnimeItem) -> FranchiseCatalog {
    let classification = classification_for_format(&release.anime_type);
    let conceptual_season = (classification == ReleaseClassification::MainlineSeason).then(|| {
        title_candidates_for_item(&release)
            .find_map(parse_season)
            .unwrap_or(1)
    });
    let part = (classification == ReleaseClassification::MainlineSeason).then(|| {
        title_candidates_for_item(&release)
            .find_map(parse_part)
            .unwrap_or(1)
    });
    let entry = ReleaseEntry {
        anihub_id: Some(release.id),
        anilist_id: None,
        title: release.title_ukrainian.clone(),
        anime_type: release.anime_type,
        year: release.year,
        poster_url: release.poster_url.clone(),
        episodes_count: release.episodes_count,
        available_episodes: release.episodes_count,
        description: release.description,
        rating: release.rating,
        genres: release.genres,
        dubbing_studios: release.dubbing_studios,
        conceptual_season,
        part,
        classification,
        availability: ReleaseAvailability::Available,
    };
    FranchiseCatalog {
        anchor_anilist_id: None,
        canonical_title: entry.title.clone(),
        canonical_poster_url: entry.poster_url.clone(),
        unresolved_anilist_ids: Vec::new(),
        releases: vec![entry],
    }
}

fn topological_mainline_order(
    mainline: &BTreeSet<u32>,
    edges: &BTreeSet<(u32, u32)>,
    nodes: &BTreeMap<u32, NodeRecord>,
) -> Vec<u32> {
    let mut indegree = mainline
        .iter()
        .map(|id| (*id, 0usize))
        .collect::<HashMap<_, _>>();
    let mut outgoing = HashMap::<u32, BTreeSet<u32>>::new();
    for &(from, to) in edges {
        if from == to || !mainline.contains(&from) || !mainline.contains(&to) {
            continue;
        }
        if outgoing.entry(from).or_default().insert(to) {
            *indegree.entry(to).or_default() += 1;
        }
    }
    let mut ready = BTreeSet::new();
    for (&id, &degree) in &indegree {
        if degree == 0 {
            let node = nodes.get(&id).expect("mainline node exists");
            ready.insert((node_sort_key(node), id));
        }
    }
    let mut ordered = Vec::new();
    while let Some((key, id)) = ready.pop_first() {
        let _ = key;
        ordered.push(id);
        if let Some(next_ids) = outgoing.get(&id) {
            for &next_id in next_ids {
                let degree = indegree.get_mut(&next_id).expect("known mainline node");
                *degree -= 1;
                if *degree == 0 {
                    let next = nodes.get(&next_id).expect("mainline node exists");
                    ready.insert((node_sort_key(next), next_id));
                }
            }
        }
    }
    if ordered.len() != mainline.len() {
        let mut remainder = mainline
            .iter()
            .copied()
            .filter(|id| !ordered.contains(id))
            .collect::<Vec<_>>();
        remainder.sort_by_key(|id| node_sort_key(nodes.get(id).expect("mainline node exists")));
        ordered.extend(remainder);
    }
    ordered
}

fn classify(node: &NodeRecord, local: Option<&AnimeItem>, mainline: bool) -> ReleaseClassification {
    if !mainline {
        return ReleaseClassification::Extra;
    }
    local
        .map(|release| classification_for_format(&release.anime_type))
        .filter(|classification| *classification != ReleaseClassification::MainlineSeason)
        .unwrap_or_else(|| classification_for_format(node.format.as_deref().unwrap_or_default()))
}

fn aired_episode_count(node: &NodeRecord, local: Option<&AnimeItem>) -> Option<u32> {
    if let Some(next_episode) = node.next_airing_episode {
        return Some(next_episode.saturating_sub(1));
    }
    node.status
        .as_deref()
        .is_some_and(|status| {
            matches!(
                normalize_token(status).as_str(),
                "FINISHED" | "COMPLETED" | "CANCELLED"
            )
        })
        // For finished releases AniHub can intentionally include a provider
        // episode that AniList does not count (for example an embedded OVA).
        // Trust the local release count so only ongoing titles are narrowed by
        // AniList's airing schedule.
        .then_some(
            local
                .and_then(|release| release.episodes_count)
                .or(node.episodes),
        )
        .flatten()
}

fn classification_for_format(format: &str) -> ReleaseClassification {
    match normalize_token(format).as_str() {
        "MOVIE" | "FILM" => ReleaseClassification::MainlineMovie,
        "SPECIAL" | "OVA" => ReleaseClassification::MainlineSpecial,
        _ => ReleaseClassification::MainlineSeason,
    }
}

fn mainline_direction(relation_type: &str) -> Option<MainlineDirection> {
    match normalize_token(relation_type).as_str() {
        "SEQUEL" => Some(MainlineDirection::CurrentBeforeNode),
        "PREQUEL" => Some(MainlineDirection::NodeBeforeCurrent),
        _ => None,
    }
}

fn is_anime(media_type: Option<&str>) -> bool {
    media_type.is_none_or(|kind| normalize_token(kind) == "ANIME")
}

fn normalize_token(value: &str) -> String {
    value.trim().replace([' ', '-'], "_").to_ascii_uppercase()
}

fn title_candidates<'a>(
    node: &'a NodeRecord,
    release: Option<&'a AnimeItem>,
) -> impl Iterator<Item = &'a str> {
    release
        .into_iter()
        .flat_map(title_candidates_for_item)
        .chain(node.title.candidates())
}

fn title_candidates_for_item(release: &AnimeItem) -> impl Iterator<Item = &str> {
    std::iter::once(release.title_english.as_deref())
        .chain(std::iter::once(release.title_original.as_deref()))
        .chain(std::iter::once(Some(release.title_ukrainian.as_str())))
        .flatten()
}

fn release_title(node: &NodeRecord, release: Option<&AnimeItem>) -> String {
    release
        .map(|release| release.title_ukrainian.clone())
        .filter(|title| !title.trim().is_empty())
        .or_else(|| node.title.display_name().map(str::to_string))
        .unwrap_or_else(|| format!("AniList #{}", node.id))
}

fn parse_season(title: &str) -> Option<u32> {
    SEASON_RE
        .captures(title)
        .or_else(|| ORDINAL_SEASON_RE.captures(title))
        .and_then(|captures| captures.get(1))
        .and_then(|number| number.as_str().parse().ok())
        .or_else(|| {
            ROMAN_SEASON_RE
                .captures(title)
                .and_then(|captures| captures.get(1))
                .and_then(|roman| roman_to_u32(roman.as_str()))
        })
}

fn parse_part(title: &str) -> Option<u32> {
    PART_RE
        .captures(title)
        .or_else(|| NUMBERED_PART_RE.captures(title))
        .and_then(|captures| captures.get(1))
        .and_then(|number| number.as_str().parse().ok())
}

fn roman_to_u32(value: &str) -> Option<u32> {
    match value.to_ascii_uppercase().as_str() {
        "II" => Some(2),
        "III" => Some(3),
        "IV" => Some(4),
        "V" => Some(5),
        "VI" => Some(6),
        "VII" => Some(7),
        "VIII" => Some(8),
        "IX" => Some(9),
        "X" => Some(10),
        _ => None,
    }
}

fn node_sort_key(node: &NodeRecord) -> (u32, u32) {
    (node.year.unwrap_or(u32::MAX), node.id)
}

fn catalog_sort_key(catalog: &FranchiseCatalog) -> (u32, u32, &str) {
    let first = catalog.releases.first();
    (
        first.and_then(|release| release.year).unwrap_or(u32::MAX),
        first
            .and_then(|release| release.anihub_id)
            .unwrap_or(u32::MAX),
        catalog.canonical_title.as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(
        id: u32,
        anilist_id: u32,
        title: &str,
        english: &str,
        anime_type: &str,
        year: u32,
    ) -> AnimeItem {
        AnimeItem {
            id,
            anilist_id: Some(anilist_id),
            slug: id.to_string(),
            title_ukrainian: title.to_string(),
            title_original: None,
            title_english: Some(english.to_string()),
            status: "completed".to_string(),
            anime_type: anime_type.to_string(),
            year: Some(year),
            has_ukrainian_dub: true,
            poster_url: Some(format!("poster-{id}")),
            episodes_count: Some(12),
            description: None,
            rating: None,
            genres: None,
            dubbing_studios: None,
        }
    }

    fn node(id: u32, title: &str, format: &str, year: u32) -> AniListMediaNode {
        AniListMediaNode {
            id,
            media_type: Some("ANIME".to_string()),
            format: Some(format.to_string()),
            status: Some("FINISHED".to_string()),
            episodes: Some(12),
            season_year: Some(year),
            next_airing_episode: None,
            title: AniListTitle {
                english: Some(title.to_string()),
                ..AniListTitle::default()
            },
            cover_image: AniListCoverImage {
                large: Some(format!("anilist-poster-{id}")),
            },
        }
    }

    fn media(node: AniListMediaNode, relations: Vec<(&str, AniListMediaNode)>) -> AniListMedia {
        AniListMedia {
            id: node.id,
            media_type: node.media_type,
            format: node.format,
            status: node.status,
            episodes: node.episodes,
            season_year: node.season_year,
            next_airing_episode: node.next_airing_episode,
            title: node.title,
            cover_image: node.cover_image,
            relations: AniListRelations {
                edges: relations
                    .into_iter()
                    .map(|(relation_type, node)| AniListRelationEdge {
                        relation_type: relation_type.to_string(),
                        node,
                    })
                    .collect(),
            },
        }
    }

    fn release(catalog: &FranchiseCatalog, anihub_id: u32) -> &ReleaseEntry {
        catalog
            .releases
            .iter()
            .find(|release| release.anihub_id == Some(anihub_id))
            .expect("fixture release")
    }

    #[test]
    fn mushoku_maps_cours_to_conceptual_seasons_and_keeps_special_as_extra() {
        let mut s1_part_2 = item(
            4887,
            127720,
            "Mushoku S1 P2",
            "Mushoku Tensei Part 2",
            "tv",
            2021,
        );
        s1_part_2.episodes_count = Some(13);
        let releases = vec![
            item(5048, 108465, "Mushoku S1", "Mushoku Tensei", "tv", 2021),
            s1_part_2,
            item(
                5180,
                146065,
                "Mushoku S2",
                "Mushoku Tensei Season 2",
                "tv",
                2023,
            ),
            item(
                4986,
                166873,
                "Mushoku S2 P2",
                "Mushoku Tensei Season 2 Part 2",
                "tv",
                2024,
            ),
            item(
                24675,
                178789,
                "Mushoku S3",
                "Mushoku Tensei Season 3",
                "tv",
                2026,
            ),
            item(
                5895,
                141534,
                "Eris",
                "Mushoku Tensei: Eris the Goblin Slayer",
                "special",
                2022,
            ),
        ];
        let n1 = node(108465, "Mushoku Tensei", "TV", 2021);
        let n1p2 = node(127720, "Mushoku Tensei Part 2", "TV", 2021);
        let n2 = node(146065, "Mushoku Tensei Season 2", "TV", 2023);
        let n2p2 = node(166873, "Mushoku Tensei Season 2 Part 2", "TV", 2024);
        let mut n3 = node(178789, "Mushoku Tensei Season 3", "TV", 2026);
        n3.status = Some("RELEASING".to_string());
        n3.episodes = Some(14);
        n3.next_airing_episode = Some(AniListNextAiringEpisode {
            episode: 4,
            airing_at: None,
        });
        let extra = node(141534, "Eris the Goblin Slayer", "SPECIAL", 2022);
        let graph = vec![
            media(
                n1.clone(),
                vec![("SEQUEL", n1p2.clone()), ("SIDE_STORY", extra.clone())],
            ),
            media(n1p2.clone(), vec![("SEQUEL", n2.clone())]),
            media(n2.clone(), vec![("SEQUEL", n2p2.clone())]),
            media(n2p2.clone(), vec![("SEQUEL", n3.clone())]),
        ];

        let catalogs = build_franchise_catalogs(&releases, &graph);
        assert_eq!(catalogs.len(), 1);
        let catalog = &catalogs[0];
        assert_eq!(catalog.canonical_poster_url.as_deref(), Some("poster-5048"));
        assert_eq!(release(catalog, 5048).conceptual_season, Some(1));
        assert_eq!(release(catalog, 5048).part, Some(1));
        assert_eq!(release(catalog, 4887).conceptual_season, Some(1));
        assert_eq!(release(catalog, 4887).part, Some(2));
        assert_eq!(release(catalog, 4887).available_episodes, Some(13));
        assert_eq!(release(catalog, 5180).conceptual_season, Some(2));
        assert_eq!(release(catalog, 5180).part, Some(1));
        assert_eq!(release(catalog, 4986).conceptual_season, Some(2));
        assert_eq!(release(catalog, 4986).part, Some(2));
        assert_eq!(release(catalog, 24675).conceptual_season, Some(3));
        assert_eq!(release(catalog, 24675).available_episodes, Some(3));
        assert_eq!(
            release(catalog, 5895).classification,
            ReleaseClassification::Extra
        );
    }

    #[test]
    fn kaguya_keeps_sequel_movie_mainline_and_ova_extra() {
        let s1 = node(101921, "Kaguya-sama: Love is War", "TV", 2019);
        let s2 = node(112641, "Kaguya-sama: Love is War Season 2", "TV", 2020);
        let s3 = node(125367, "Kaguya-sama: Ultra Romantic", "TV", 2022);
        let movie = node(151384, "The First Kiss That Never Ends", "MOVIE", 2022);
        let special = node(194884, "Stairway to Adulthood", "SPECIAL", 2025);
        let ova = node(134496, "Kaguya-sama OVA", "OVA", 2021);
        let available = vec![
            item(1, s1.id, "Kaguya", "Kaguya-sama: Love is War", "tv", 2019),
            item(2, s2.id, "Kaguya 2", "Kaguya-sama Season 2", "tv", 2020),
            item(
                3,
                s3.id,
                "Kaguya 3",
                "Kaguya-sama Ultra Romantic",
                "tv",
                2022,
            ),
            item(
                4,
                movie.id,
                "Перший поцілунок",
                "The First Kiss",
                "movie",
                2022,
            ),
            item(5, ova.id, "OVA", "Kaguya-sama OVA", "ova", 2021),
            item(
                6,
                special.id,
                "Сходи в доросле життя",
                "Stairway to Adulthood",
                "special",
                2025,
            ),
        ];
        let graph = vec![
            media(s1.clone(), vec![("SEQUEL", s2.clone())]),
            media(
                s2.clone(),
                vec![("SEQUEL", s3.clone()), ("SIDE_STORY", ova.clone())],
            ),
            media(s3, vec![("SEQUEL", movie.clone())]),
            media(movie, vec![("SEQUEL", special)]),
        ];
        let catalog = &build_franchise_catalogs(&available, &graph)[0];
        assert_eq!(
            release(catalog, 4).classification,
            ReleaseClassification::MainlineMovie
        );
        assert_eq!(
            release(catalog, 6).classification,
            ReleaseClassification::MainlineSpecial
        );
        assert_eq!(
            release(catalog, 5).classification,
            ReleaseClassification::Extra
        );
        assert_eq!(release(catalog, 3).conceptual_season, Some(3));
        assert!(catalog.unresolved_anilist_ids.is_empty());
    }

    #[test]
    fn hidden_kaguya_sequel_remains_discoverable_by_exact_lookup() {
        let s1 = node(101921, "Kaguya-sama: Love is War", "TV", 2019);
        let s2 = node(112641, "Kaguya-sama: Love is War Season 2", "TV", 2020);
        let s3 = node(125367, "Kaguya-sama: Ultra Romantic", "TV", 2022);
        let graph = vec![
            media(s1.clone(), vec![("SEQUEL", s2.clone())]),
            media(s2.clone(), vec![("SEQUEL", s3.clone())]),
        ];
        let mut available = vec![
            item(1, s1.id, "Kaguya", "Kaguya-sama: Love is War", "tv", 2019),
            item(2, s2.id, "Kaguya 2", "Kaguya-sama Season 2", "tv", 2020),
        ];

        let catalog = &build_franchise_catalogs(&available, &graph)[0];
        assert!(
            catalog
                .releases
                .iter()
                .all(|release| release.anilist_id != Some(s3.id))
        );
        assert_eq!(catalog.unresolved_anilist_ids, vec![s3.id]);

        available.push(item(
            3,
            s3.id,
            "Kaguya 3",
            "Kaguya-sama Ultra Romantic",
            "tv",
            2022,
        ));
        let catalog = &build_franchise_catalogs(&available, &graph)[0];
        assert_eq!(release(catalog, 3).conceptual_season, Some(3));
        assert!(catalog.unresolved_anilist_ids.is_empty());
    }

    #[test]
    fn assassination_classroom_hides_all_unavailable_relations() {
        let s1 = node(20755, "Assassination Classroom", "TV", 2015);
        let s2 = node(21170, "Assassination Classroom 2nd Season", "TV", 2016);
        let summary = node(21878, "Assassination Classroom: 365 Days", "MOVIE", 2016);
        let available = vec![item(
            10,
            s1.id,
            "Клас убивць",
            "Assassination Classroom",
            "tv",
            2015,
        )];
        let graph = vec![media(
            s1,
            vec![("SEQUEL", s2.clone()), ("SUMMARY", summary.clone())],
        )];
        let catalog = &build_franchise_catalogs(&available, &graph)[0];
        assert!(
            catalog
                .releases
                .iter()
                .all(|release| release.anilist_id != Some(s2.id))
        );
        assert!(
            catalog
                .releases
                .iter()
                .all(|release| release.anilist_id != Some(summary.id))
        );
        assert_eq!(catalog.unresolved_anilist_ids, vec![s2.id]);
    }

    #[test]
    fn anthology_special_does_not_merge_unrelated_franchises() {
        let assassination_s1 = node(20755, "Assassination Classroom", "TV", 2015);
        let assassination_s2 = node(21170, "Assassination Classroom 2nd Season", "TV", 2016);
        let assassination_ova = node(30001, "Assassination Classroom OVA", "OVA", 2015);
        let jump_festa = node(30002, "Jump Festa Omnibus", "SPECIAL", 2013);
        let kuroko = node(11771, "Kuroko's Basketball", "TV", 2012);
        let available = vec![
            item(
                10,
                assassination_s1.id,
                "Клас убивць",
                "Assassination Classroom",
                "tv",
                2015,
            ),
            item(
                11,
                assassination_s2.id,
                "Клас убивць 2",
                "Assassination Classroom 2nd Season",
                "tv",
                2016,
            ),
            item(
                12,
                assassination_ova.id,
                "Клас убивць OVA",
                "Assassination Classroom OVA",
                "ova",
                2015,
            ),
            item(
                13,
                kuroko.id,
                "Баскетбол Куроко",
                "Kuroko's Basketball",
                "tv",
                2012,
            ),
        ];
        let graph = vec![
            media(
                assassination_s1.clone(),
                vec![
                    ("SEQUEL", assassination_s2.clone()),
                    ("SIDE_STORY", assassination_ova.clone()),
                    ("OTHER", jump_festa.clone()),
                ],
            ),
            media(kuroko.clone(), vec![("OTHER", jump_festa.clone())]),
        ];

        let catalogs = build_franchise_catalogs(&available, &graph);
        let assassination = catalogs
            .iter()
            .find(|catalog| {
                catalog
                    .releases
                    .iter()
                    .any(|release| release.anilist_id == Some(assassination_s1.id))
            })
            .unwrap();

        assert!(
            assassination
                .releases
                .iter()
                .any(|release| release.anilist_id == Some(assassination_ova.id))
        );
        assert!(
            assassination
                .releases
                .iter()
                .all(|release| release.anilist_id != Some(kuroko.id))
        );
        assert!(
            assassination
                .releases
                .iter()
                .all(|release| release.anilist_id != Some(jump_festa.id))
        );
        assert!(catalogs.iter().any(|catalog| {
            catalog.releases.len() == 1 && catalog.releases[0].anilist_id == Some(kuroko.id)
        }));
    }

    #[test]
    fn unrelated_prefix_titles_do_not_group_without_anilist_edges() {
        let first = item(20, 1, "Монстр", "Monster", "tv", 2004);
        let second = item(21, 2, "Монстр №8", "Kaiju No. 8", "tv", 2024);
        let graph = vec![
            media(node(1, "Monster", "TV", 2004), Vec::new()),
            media(node(2, "Kaiju No. 8", "TV", 2024), Vec::new()),
        ];
        let catalogs = build_franchise_catalogs(&[first, second], &graph);
        assert_eq!(catalogs.len(), 2);
        assert!(catalogs.iter().all(|catalog| catalog.releases.len() == 1));
    }
}
