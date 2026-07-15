use serde::{Deserialize, Serialize};

/// Exact AniHub source query. The same anime id can expose different episode
/// sets depending on the franchise-level `season` query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EpisodeSourcesKey {
    pub anime_id: u32,
    pub season: u32,
}

impl EpisodeSourcesKey {
    pub const fn new(anime_id: u32, season: u32) -> Self {
        Self { anime_id, season }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AnimeSearchResponse {
    pub total: u32,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    pub items: Vec<AnimeItem>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct AnimeItem {
    pub id: u32,
    pub anilist_id: Option<u32>,
    pub slug: String,
    pub title_ukrainian: String,
    pub title_original: Option<String>,
    pub title_english: Option<String>,
    pub status: String,
    #[serde(rename = "type")]
    pub anime_type: String,
    pub year: Option<u32>,
    pub has_ukrainian_dub: bool,
    /// Search responses did not always include poster metadata.  Keeping this
    /// optional lets older fixtures and partial API responses deserialize.
    #[serde(default)]
    pub poster_url: Option<String>,
    /// Number of episodes reported for this particular AniHub release.
    #[serde(default)]
    pub episodes_count: Option<u32>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub rating: Option<f32>,
    #[serde(default)]
    pub genres: Option<Vec<String>>,
    #[serde(default)]
    pub dubbing_studios: Option<Vec<DubbingStudio>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct EpisodeSourcesResponse {
    pub ashdi: Vec<AshdiStudio>,
    #[serde(default)]
    pub moonanime: Vec<MoonAnimeSourceMarker>,
}

/// Minimal browser-only MoonAnime metadata. Episode/iframe payloads are
/// intentionally ignored; the TUI only needs the dubbing label and count.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct MoonAnimeSourceMarker {
    #[serde(default)]
    pub studio_name: String,
    #[serde(default)]
    pub season_number: u32,
    #[serde(default)]
    pub episodes_count: u32,
    #[serde(default)]
    pub episodes: Vec<MoonAnimeBrowserEpisode>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct MoonAnimeBrowserEpisode {
    pub episode_number: u32,
    #[serde(default)]
    pub display_episode_number: Option<f32>,
    #[serde(default)]
    pub title: String,
    pub iframe_url: String,
}

impl EpisodeSourcesResponse {
    pub fn is_moonanime_only(&self) -> bool {
        self.ashdi.is_empty() && !self.moonanime.is_empty()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct AshdiStudio {
    pub id: u32,
    pub studio_name: String,
    pub season_number: u32,
    pub episodes: Vec<AshdiEpisode>,
    pub episodes_count: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct AshdiEpisode {
    pub episode_number: u32,
    pub display_episode_number: Option<f32>,
    pub title: String,
    pub url: String,
    pub ashdi_episode_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AnimeDetails {
    pub id: u32,
    pub anilist_id: Option<u32>,
    pub slug: String,
    pub title_ukrainian: String,
    pub title_original: Option<String>,
    pub title_english: Option<String>,
    pub status: String,
    #[serde(rename = "type")]
    pub anime_type: String,
    pub year: Option<u32>,
    pub has_ukrainian_dub: bool,
    pub poster_url: Option<String>,
    pub episodes_count: Option<u32>,
    pub description: Option<String>,
    pub rating: Option<f32>,
    pub genres: Option<Vec<String>>,
    pub dubbing_studios: Option<Vec<DubbingStudio>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct DubbingStudio {
    pub id: u32,
    pub name: String,
    pub slug: String,
}
