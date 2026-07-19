//! Shared release, dubbing, and episode panel state.
//!
//! Search and Library have different root lists, but intentionally converge on
//! the same three content panels. This state therefore has one owner instead
//! of being duplicated per tab or scattered across `AppState`.

use crate::api::{AnimeDetails, EpisodeSourcesKey, EpisodeSourcesResponse};
use ratatui::widgets::ListState;

#[derive(Default)]
pub struct ContentUiState {
    pub selected_season_index: Option<usize>,
    pub selected_dubbing_index: Option<usize>,
    pub selected_episode_index: Option<usize>,
    pub current_sources: Option<EpisodeSourcesResponse>,
    pub current_sources_key: Option<EpisodeSourcesKey>,
    pub current_details: Option<AnimeDetails>,
    pub studio_anime_ids: Vec<u32>,
    pub sidebar_anime_idx: Option<usize>,
    pub sidebar_subject_id: Option<u32>,
    pub season_list_state: ListState,
    pub dubbing_list_state: ListState,
    pub episode_list_state: ListState,
}

impl ContentUiState {
    /// Drop release/dubbing/episode ownership while preserving the currently
    /// rendered title details and sidebar anchor.
    pub fn clear_drilldown(&mut self) {
        self.selected_season_index = None;
        self.selected_dubbing_index = None;
        self.selected_episode_index = None;
        self.season_list_state.select(None);
        self.dubbing_list_state.select(None);
        self.episode_list_state.select(None);
        self.current_sources = None;
        self.current_sources_key = None;
        self.studio_anime_ids.clear();
    }

    /// A new root title must not inherit any content or sidebar ownership from
    /// the previously highlighted search result.
    pub fn clear_for_new_root(&mut self) {
        self.clear_drilldown();
        self.current_details = None;
        self.sidebar_anime_idx = None;
        self.sidebar_subject_id = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn populated_state() -> ContentUiState {
        let mut state = ContentUiState {
            selected_season_index: Some(2),
            selected_dubbing_index: Some(1),
            selected_episode_index: Some(8),
            current_sources: Some(EpisodeSourcesResponse {
                ashdi: Vec::new(),
                moonanime: Vec::new(),
            }),
            current_sources_key: Some(EpisodeSourcesKey::new(42, 3)),
            current_details: Some(AnimeDetails {
                id: 42,
                anilist_id: Some(4242),
                slug: "test".to_string(),
                title_ukrainian: "Тест".to_string(),
                title_original: None,
                title_english: None,
                status: "finished".to_string(),
                anime_type: "TV".to_string(),
                year: Some(2024),
                has_ukrainian_dub: true,
                poster_url: None,
                episodes_count: Some(12),
                description: None,
                rating: None,
                genres: None,
                dubbing_studios: None,
            }),
            studio_anime_ids: vec![42],
            sidebar_anime_idx: Some(4),
            sidebar_subject_id: Some(42),
            ..ContentUiState::default()
        };
        state.season_list_state.select(Some(2));
        state.dubbing_list_state.select(Some(1));
        state.episode_list_state.select(Some(8));
        state
    }

    #[test]
    fn collapsing_drilldown_keeps_root_sidebar_ownership() {
        let mut state = populated_state();
        state.clear_drilldown();

        assert_eq!(state.sidebar_anime_idx, Some(4));
        assert_eq!(state.sidebar_subject_id, Some(42));
        assert!(state.current_details.is_some());
        assert!(state.current_sources.is_none());
        assert!(state.studio_anime_ids.is_empty());
        assert_eq!(state.season_list_state.selected(), None);
        assert_eq!(state.dubbing_list_state.selected(), None);
        assert_eq!(state.episode_list_state.selected(), None);
    }

    #[test]
    fn selecting_a_new_root_clears_every_downstream_owner() {
        let mut state = populated_state();
        state.clear_for_new_root();

        assert!(state.current_sources.is_none());
        assert!(state.current_sources_key.is_none());
        assert!(state.current_details.is_none());
        assert!(state.studio_anime_ids.is_empty());
        assert!(state.sidebar_anime_idx.is_none());
        assert!(state.sidebar_subject_id.is_none());
        assert!(state.selected_season_index.is_none());
        assert!(state.selected_dubbing_index.is_none());
        assert!(state.selected_episode_index.is_none());
    }
}
