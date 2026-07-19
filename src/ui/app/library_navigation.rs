//! Library drill-down navigation.
//!
//! This module owns the transitions between anime, release, dubbing, and
//! episode lists. Keeping the index arithmetic here makes key dispatch and the
//! broader application state independent of the navigation details.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Up,
    Down,
}

fn wrapped_index(selected: Option<usize>, total: usize, direction: Direction) -> Option<usize> {
    if total == 0 {
        return None;
    }

    Some(match (selected, direction) {
        (None, _) => 0,
        (Some(0), Direction::Up) => total - 1,
        (Some(index), Direction::Up) => index.saturating_sub(1).min(total - 1),
        (Some(index), Direction::Down) if index >= total - 1 => 0,
        (Some(index), Direction::Down) => index + 1,
    })
}

const fn parent_mode(mode: AppMode) -> Option<AppMode> {
    match mode {
        AppMode::LibraryEpisode => Some(AppMode::LibraryDubbing),
        AppMode::LibraryDubbing => Some(AppMode::LibrarySeason),
        AppMode::LibrarySeason => Some(AppMode::Library),
        _ => None,
    }
}

impl AppState {
    pub(super) fn prepare_library_anime_selection(&mut self) {
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
        self.content.studio_anime_ids.clear();
        self.sync_library_sidebar_selection();
        self.loading = true;
        self.activity_message = Some("Завантаження бібліотеки…".to_string());
    }

    pub(super) fn leave_library_level(&mut self) {
        match parent_mode(self.mode) {
            Some(AppMode::LibraryDubbing) => {
                self.mode = AppMode::LibraryDubbing;
                self.content.selected_episode_index = None;
                self.content.episode_list_state.select(None);
            }
            Some(AppMode::LibrarySeason) => {
                self.mode = AppMode::LibrarySeason;
                self.content.selected_dubbing_index = None;
                self.content.dubbing_list_state.select(None);
                self.content.selected_episode_index = None;
                self.content.episode_list_state.select(None);
            }
            Some(AppMode::Library) => {
                self.mode = AppMode::Library;
                self.content.selected_season_index = None;
                self.content.selected_dubbing_index = None;
                self.content.selected_episode_index = None;
                self.content.season_list_state.select(None);
                self.content.dubbing_list_state.select(None);
                self.content.episode_list_state.select(None);
            }
            None if self.mode == AppMode::Library => self.reset_to_home(),
            None | Some(_) => {}
        }
    }

    pub(super) fn enter_library_season(&mut self) {
        if self.library_selected_anime().is_none()
            || (self.unique_seasons().is_empty()
                && self
                    .library_selected_anime()
                    .is_none_or(|anime| anime.seasons.is_empty()))
        {
            return;
        }

        self.mode = AppMode::LibrarySeason;
        self.content.selected_season_index = Some(0);
        self.content.selected_dubbing_index = None;
        self.content.selected_episode_index = None;
        self.content.season_list_state.select(Some(0));
        self.content.dubbing_list_state.select(None);
        self.content.episode_list_state.select(None);
        self.sync_library_sidebar_selection();
    }

    pub(super) fn enter_library_dubbing(&mut self) {
        let Some(season_num) = self.selected_season_num() else {
            return;
        };
        if self.dubbing_choices_for_season(season_num).is_empty() {
            return;
        }
        self.acknowledge_selected_library_release();

        self.mode = AppMode::LibraryDubbing;
        self.content.selected_dubbing_index = Some(0);
        self.content.selected_episode_index = None;
        self.content.dubbing_list_state.select(Some(0));
        self.content.episode_list_state.select(None);
    }

    pub(super) fn enter_library_episode(&mut self) {
        if self.selected_episode_count() == 0 {
            return;
        }
        self.mode = AppMode::LibraryEpisode;
        self.content.selected_episode_index = Some(0);
        self.content.episode_list_state.select(Some(0));
    }

    pub(super) fn move_library_down(&mut self) {
        self.move_library_selection(Direction::Down);
    }

    pub(super) fn move_library_up(&mut self) {
        self.move_library_selection(Direction::Up);
    }

    fn move_library_selection(&mut self, direction: Direction) {
        match self.mode {
            AppMode::Library => {
                let Some(next) = wrapped_index(
                    self.library.anime_list_state.selected(),
                    self.library.items.len(),
                    direction,
                ) else {
                    return;
                };
                self.library.anime_index = Some(next);
                self.library.anime_list_state.select(Some(next));
                self.remember_library_selection();
                self.prepare_library_anime_selection();
            }
            AppMode::LibrarySeason => {
                let Some(next) = wrapped_index(
                    self.content.season_list_state.selected(),
                    self.library_season_numbers().len(),
                    direction,
                ) else {
                    return;
                };
                self.content.selected_season_index = Some(next);
                self.content.season_list_state.select(Some(next));
                self.sync_library_sidebar_selection();
            }
            AppMode::LibraryDubbing => {
                let Some(season_num) = self.selected_season_num() else {
                    return;
                };
                let Some(next) = wrapped_index(
                    self.content.dubbing_list_state.selected(),
                    self.dubbing_choices_for_season(season_num).len(),
                    direction,
                ) else {
                    return;
                };
                self.content.selected_dubbing_index = Some(next);
                self.content.dubbing_list_state.select(Some(next));
            }
            AppMode::LibraryEpisode => {
                let Some(next) = wrapped_index(
                    self.content.episode_list_state.selected(),
                    self.selected_episode_count(),
                    direction,
                ) else {
                    return;
                };
                self.content.selected_episode_index = Some(next);
                self.content.episode_list_state.select(Some(next));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_navigation_handles_empty_initial_and_edges() {
        assert_eq!(wrapped_index(None, 0, Direction::Down), None);
        assert_eq!(wrapped_index(None, 3, Direction::Down), Some(0));
        assert_eq!(wrapped_index(None, 3, Direction::Up), Some(0));
        assert_eq!(wrapped_index(Some(0), 3, Direction::Up), Some(2));
        assert_eq!(wrapped_index(Some(2), 3, Direction::Down), Some(0));
        assert_eq!(wrapped_index(Some(1), 3, Direction::Up), Some(0));
        assert_eq!(wrapped_index(Some(1), 3, Direction::Down), Some(2));
    }

    #[test]
    fn library_parent_modes_follow_the_visible_drill_down() {
        assert_eq!(
            parent_mode(AppMode::LibraryEpisode),
            Some(AppMode::LibraryDubbing)
        );
        assert_eq!(
            parent_mode(AppMode::LibraryDubbing),
            Some(AppMode::LibrarySeason)
        );
        assert_eq!(parent_mode(AppMode::LibrarySeason), Some(AppMode::Library));
        assert_eq!(parent_mode(AppMode::Library), None);
        assert_eq!(parent_mode(AppMode::Normal), None);
    }
}
