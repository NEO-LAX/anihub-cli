//! Search drill-down and list navigation.
//!
//! This module owns transitions between franchise results, releases,
//! dubbings, and episodes. The methods intentionally keep the existing
//! `AppState` facade while removing transition logic from the root module.

use super::*;

impl AppState {
    pub(super) fn handle_esc(&mut self) {
        if self.is_library_mode() {
            self.leave_library_level();
            return;
        }
        if matches!(self.status_message, Some((_, StatusKind::Error))) {
            self.clear_status();
            return;
        }
        match self.focus {
            FocusPanel::EpisodeList => self.focus = FocusPanel::DubbingList,
            FocusPanel::DubbingList => {
                if self.has_release_catalog() {
                    self.focus = FocusPanel::ReleaseList;
                } else if self.unique_seasons().len() <= 1 {
                    self.collapse_search_drilldown();
                } else {
                    self.focus = FocusPanel::ReleaseList;
                }
            }
            FocusPanel::ReleaseList => {
                self.collapse_search_drilldown();
            }
            FocusPanel::SearchList => {
                // Residual drill-down left open → collapse first, keep results.
                if self.search.selected_release_index.is_some()
                    || self.content.selected_season_index.is_some()
                    || self.content.selected_dubbing_index.is_some()
                    || self.content.current_sources.is_some()
                {
                    self.collapse_search_drilldown();
                    return;
                }
                // Already fully out of seasons/episodes: Esc clears the list.
                if !self.search.results.is_empty() || !self.search.last_query.is_empty() {
                    self.clear_search_session();
                }
            }
        }
    }

    /// Leave season/dubbing/episode columns and keep the franchise list + query.
    fn collapse_search_drilldown(&mut self) {
        self.focus = FocusPanel::SearchList;
        self.search.selected_release_index = None;
        self.content.selected_season_index = None;
        self.content.selected_dubbing_index = None;
        self.content.selected_episode_index = None;
        self.content.season_list_state.select(None);
        self.content.dubbing_list_state.select(None);
        self.content.episode_list_state.select(None);
        self.content.current_sources = None;
        self.content.current_sources_key = None;
        self.content.studio_anime_ids.clear();
        if let Some(index) = self.search.selected_group_index {
            self.search.result_list_state.select(Some(index));
        }
        self.restore_representative_poster();
    }

    /// Empty search tab → "press / to search" home.
    fn clear_search_session(&mut self) {
        self.mode = AppMode::Normal;
        self.focus = FocusPanel::SearchList;
        self.search.query.clear();
        self.search.last_query.clear();
        self.search.cursor = 0;
        self.search.results.clear();
        self.search.franchise_groups.clear();
        self.search.franchise_catalogs.clear();
        self.search.anilist_media.clear();
        self.search.selected_group_index = None;
        self.search.selected_result_index = None;
        self.search.result_list_state.select(None);
        self.search.selected_release_index = None;
        self.content.selected_season_index = None;
        self.content.selected_dubbing_index = None;
        self.content.selected_episode_index = None;
        self.content.season_list_state.select(None);
        self.content.dubbing_list_state.select(None);
        self.content.episode_list_state.select(None);
        self.content.current_sources = None;
        self.content.current_sources_key = None;
        self.content.current_details = None;
        self.current_poster = None;
        self.poster_fetch_pending = None;
        self.content.studio_anime_ids.clear();
        self.content.sidebar_anime_idx = None;
        self.content.sidebar_subject_id = None;
        self.loading = false;
        self.clear_activity();
        self.clear_status();
    }

    pub(super) fn handle_enter(&mut self) {
        if self.focus == FocusPanel::EpisodeList {
            self.activate_selected_episode();
        } else {
            self.move_focus_right();
        }
    }

    pub(super) fn move_focus_right(&mut self) {
        self.focus = match self.focus {
            FocusPanel::SearchList => {
                if self.search.selected_result_index.is_some() {
                    if self.has_release_catalog() {
                        let index = self.initial_release_index();
                        self.select_release(index);
                        FocusPanel::ReleaseList
                    } else {
                        let has_seasons = self
                            .content
                            .current_sources
                            .as_ref()
                            .is_some_and(|sources| !sources.ashdi.is_empty());
                        let seasons = self.unique_seasons();
                        if has_seasons && !seasons.is_empty() {
                            self.content.selected_season_index = Some(0);
                            self.content.season_list_state.select(Some(0));
                            self.update_sidebar_for_season();
                            if seasons.len() == 1 {
                                let season_num = seasons[0];
                                let dubbing_count =
                                    self.dubbing_choices_for_season(season_num).len();
                                if dubbing_count > 0 {
                                    self.content.selected_dubbing_index = Some(0);
                                    self.content.dubbing_list_state.select(Some(0));
                                    FocusPanel::DubbingList
                                } else {
                                    FocusPanel::ReleaseList
                                }
                            } else {
                                FocusPanel::ReleaseList
                            }
                        } else {
                            FocusPanel::SearchList
                        }
                    }
                } else {
                    FocusPanel::SearchList
                }
            }
            FocusPanel::ReleaseList => {
                if self.has_release_catalog() && !self.selected_release_available() {
                    FocusPanel::ReleaseList
                } else if let Some(season) = self.selected_season_num() {
                    let dubbing_count = self.dubbing_choices_for_season(season).len();
                    if dubbing_count > 0 {
                        self.content.selected_dubbing_index = Some(0);
                        self.content.dubbing_list_state.select(Some(0));
                        FocusPanel::DubbingList
                    } else {
                        FocusPanel::ReleaseList
                    }
                } else {
                    FocusPanel::ReleaseList
                }
            }
            FocusPanel::DubbingList => {
                if self.selected_episode_count() > 0 {
                    self.content.selected_episode_index = Some(0);
                    self.content.episode_list_state.select(Some(0));
                    FocusPanel::EpisodeList
                } else {
                    FocusPanel::DubbingList
                }
            }
            FocusPanel::EpisodeList => FocusPanel::EpisodeList,
        };
    }

    pub(super) fn move_focus_left(&mut self) {
        match self.focus {
            FocusPanel::EpisodeList => self.focus = FocusPanel::DubbingList,
            FocusPanel::DubbingList => {
                if self.has_release_catalog() {
                    self.focus = FocusPanel::ReleaseList;
                } else if self.unique_seasons().len() <= 1 {
                    self.focus = FocusPanel::SearchList;
                    self.restore_representative_poster();
                } else {
                    self.focus = FocusPanel::ReleaseList;
                }
            }
            FocusPanel::ReleaseList => {
                self.focus = FocusPanel::SearchList;
                self.restore_representative_poster();
            }
            FocusPanel::SearchList => {}
        }
    }

    fn move_release_selection(&mut self, down: bool) {
        let Some(next) = wrapped_index(
            self.content.season_list_state.selected(),
            self.release_count(),
            down,
        ) else {
            return;
        };

        if self.has_release_catalog() {
            self.select_release(Some(next));
        } else {
            self.content.season_list_state.select(Some(next));
            self.content.selected_season_index = Some(next);
            self.content.selected_dubbing_index = None;
            self.content.dubbing_list_state.select(None);
            self.update_sidebar_for_season();
        }
    }

    pub(super) fn move_selection_down(&mut self) {
        self.move_selection(true);
    }

    pub(super) fn move_selection_up(&mut self) {
        self.move_selection(false);
    }

    fn move_selection(&mut self, down: bool) {
        match self.focus {
            FocusPanel::SearchList => {
                let Some(index) = wrapped_index(
                    self.search.result_list_state.selected(),
                    self.search.franchise_groups.len(),
                    down,
                ) else {
                    return;
                };
                self.search.result_list_state.select(Some(index));
                self.search.selected_group_index = Some(index);
                if let Some(group) = self.search.franchise_groups.get(index) {
                    self.search.selected_result_index = group.first().copied();
                }
                self.reset_downstream();
            }
            FocusPanel::ReleaseList => self.move_release_selection(down),
            FocusPanel::DubbingList => {
                let Some(season) = self.selected_season_num() else {
                    return;
                };
                let dubbing_count = self.dubbing_choices_for_season(season).len();
                let Some(index) = wrapped_index(
                    self.content.dubbing_list_state.selected(),
                    dubbing_count,
                    down,
                ) else {
                    return;
                };
                self.content.dubbing_list_state.select(Some(index));
                self.content.selected_dubbing_index = Some(index);
            }
            FocusPanel::EpisodeList => {
                let Some(index) = wrapped_index(
                    self.content.episode_list_state.selected(),
                    self.selected_episode_count(),
                    down,
                ) else {
                    return;
                };
                self.content.episode_list_state.select(Some(index));
                self.content.selected_episode_index = Some(index);
            }
        }
    }

    fn reset_downstream(&mut self) {
        self.loading = true;
        self.activity_message = Some("Завантаження вибраного аніме…".to_string());
        self.content.current_sources = None;
        self.content.current_sources_key = None;
        self.content.current_details = None;
        self.current_poster = None;
        self.content.studio_anime_ids.clear();
        self.content.sidebar_anime_idx = None;
        self.content.sidebar_subject_id = None;
        self.search.selected_release_index = None;
        self.content.selected_season_index = None;
        self.content.season_list_state.select(None);
        self.content.selected_dubbing_index = None;
        self.content.dubbing_list_state.select(None);
        self.content.selected_episode_index = None;
        self.content.episode_list_state.select(None);

        // Moving the search cursor changes the poster owner immediately.
        // Leaving the subject unset would keep the previous pending request,
        // whose completion is correctly rejected as stale, but no request for
        // the newly highlighted card would ever be scheduled until Enter.
        let subject = self.canonical_sidebar_subject().or_else(|| {
            self.search
                .selected_result_index
                .and_then(|index| self.search.results.get(index))
                .map(|item| item.id)
        });
        self.select_sidebar_subject(subject);
    }
}

fn wrapped_index(selected: Option<usize>, total: usize, down: bool) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let Some(current) = selected else {
        return Some(0);
    };
    Some(if down {
        if current >= total - 1 { 0 } else { current + 1 }
    } else if current == 0 {
        total - 1
    } else {
        current - 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_navigation_handles_empty_initial_and_both_edges() {
        assert_eq!(wrapped_index(None, 0, true), None);
        assert_eq!(wrapped_index(None, 3, true), Some(0));
        assert_eq!(wrapped_index(None, 3, false), Some(0));
        assert_eq!(wrapped_index(Some(2), 3, true), Some(0));
        assert_eq!(wrapped_index(Some(0), 3, false), Some(2));
    }
}
