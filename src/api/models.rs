use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AnimeSearchResponse {
    pub total: u32,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    pub items: Vec<AnimeItem>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EpisodeSourcesResponse {
    pub ashdi: Vec<AshdiStudio>,
    #[serde(default)]
    pub moonanime: Vec<MoonAnimeStudio>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MoonAnimeStudio {
    pub id: u32,
    pub studio_name: String,
    pub season_number: u32,
    pub episodes: Vec<MoonAnimeEpisode>,
    pub episodes_count: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MoonAnimeEpisode {
    pub episode_number: u32,
    pub display_episode_number: Option<f32>,
    pub title: String,
    pub iframe_url: String,
    #[serde(default)]
    pub poster_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AshdiStudio {
    pub id: u32,
    pub studio_name: String,
    pub season_number: u32,
    pub episodes: Vec<AshdiEpisode>,
    pub episodes_count: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DubbingStudio {
    pub id: u32,
    pub name: String,
    pub slug: String,
}
