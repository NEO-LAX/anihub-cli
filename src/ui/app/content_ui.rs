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
