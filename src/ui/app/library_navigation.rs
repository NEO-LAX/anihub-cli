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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LibraryDrillDown {
    mode: AppMode,
    season: Option<usize>,
    dubbing: Option<usize>,
    episode: Option<usize>,
}

impl LibraryDrillDown {
    const fn root() -> Self {
        Self {
            mode: AppMode::Library,
            season: None,
            dubbing: None,
            episode: None,
        }
    }

    fn enter_season(&mut self, has_releases: bool) -> bool {
        if self.mode != AppMode::Library || !has_releases {
            return false;
        }
        self.mode = AppMode::LibrarySeason;
        self.season = Some(0);
        self.dubbing = None;
        self.episode = None;
        true
    }

    fn enter_dubbing(&mut self, has_dubbings: bool) -> bool {
        if self.mode != AppMode::LibrarySeason || self.season.is_none() || !has_dubbings {
            return false;
        }
        self.mode = AppMode::LibraryDubbing;
        self.dubbing = Some(0);
        self.episode = None;
        true
    }

    fn enter_episode(&mut self, has_episodes: bool) -> bool {
        if self.mode != AppMode::LibraryDubbing || self.dubbing.is_none() || !has_episodes {
            return false;
        }
        self.mode = AppMode::LibraryEpisode;
        self.episode = Some(0);
        true
    }

    fn leave(&mut self) -> bool {
        match self.mode {
            AppMode::LibraryEpisode => {
                self.mode = AppMode::LibraryDubbing;
                self.episode = None;
            }
            AppMode::LibraryDubbing => {
                self.mode = AppMode::LibrarySeason;
                self.dubbing = None;
                self.episode = None;
            }
            AppMode::LibrarySeason => *self = Self::root(),
            _ => return false,
        }
        true
    }

    const fn clear_selection(mut self) -> Self {
        self.season = None;
        self.dubbing = None;
        self.episode = None;
        self
    }
}

impl AppState {
    fn library_drill_down(&self) -> LibraryDrillDown {
        LibraryDrillDown {
            mode: self.mode,
            season: self.content.selected_season_index,
            dubbing: self.content.selected_dubbing_index,
            episode: self.content.selected_episode_index,
        }
    }

    fn apply_library_drill_down(&mut self, drill_down: LibraryDrillDown) {
        self.mode = drill_down.mode;
        self.content.selected_season_index = drill_down.season;
        self.content.selected_dubbing_index = drill_down.dubbing;
        self.content.selected_episode_index = drill_down.episode;
        self.content.season_list_state.select(drill_down.season);
        self.content.dubbing_list_state.select(drill_down.dubbing);
        self.content.episode_list_state.select(drill_down.episode);
    }

    pub(super) fn prepare_library_anime_selection(&mut self) {
        let cleared = self.library_drill_down().clear_selection();
        self.apply_library_drill_down(cleared);
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
        let mut drill_down = self.library_drill_down();
        if drill_down.leave() {
            self.apply_library_drill_down(drill_down);
        } else if self.mode == AppMode::Library {
            self.reset_to_home();
        }
    }

    pub(super) fn enter_library_season(&mut self) {
        let has_releases = self
            .library_selected_anime()
            .is_some_and(|anime| !self.unique_seasons().is_empty() || !anime.seasons.is_empty());
        let mut drill_down = self.library_drill_down();
        if !drill_down.enter_season(has_releases) {
            return;
        }

        self.apply_library_drill_down(drill_down);
        self.sync_library_sidebar_selection();
    }

    pub(super) fn enter_library_dubbing(&mut self) {
        let Some(season_num) = self.selected_season_num() else {
            return;
        };
        let mut drill_down = self.library_drill_down();
        if !drill_down.enter_dubbing(!self.dubbing_choices_for_season(season_num).is_empty()) {
            return;
        }
        self.acknowledge_selected_library_release();

        self.apply_library_drill_down(drill_down);
    }

    pub(super) fn enter_library_episode(&mut self) {
        let mut drill_down = self.library_drill_down();
        if !drill_down.enter_episode(self.selected_episode_count() > 0) {
            return;
        }
        self.apply_library_drill_down(drill_down);
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
    fn library_drill_down_round_trips_and_clears_child_selections() {
        let mut navigation = LibraryDrillDown::root();

        assert!(navigation.enter_season(true));
        assert_eq!(navigation.mode, AppMode::LibrarySeason);
        assert_eq!(navigation.season, Some(0));
        assert!(navigation.enter_dubbing(true));
        assert_eq!(navigation.mode, AppMode::LibraryDubbing);
        assert_eq!(navigation.dubbing, Some(0));
        assert!(navigation.enter_episode(true));
        assert_eq!(navigation.mode, AppMode::LibraryEpisode);
        assert_eq!(navigation.episode, Some(0));

        assert!(navigation.leave());
        assert_eq!(navigation.mode, AppMode::LibraryDubbing);
        assert_eq!(navigation.episode, None);
        assert!(navigation.leave());
        assert_eq!(navigation.mode, AppMode::LibrarySeason);
        assert_eq!(navigation.dubbing, None);
        assert!(navigation.leave());
        assert_eq!(navigation, LibraryDrillDown::root());
        assert!(!navigation.leave());
    }

    #[test]
    fn library_drill_down_rejects_missing_content_without_partial_state() {
        let mut navigation = LibraryDrillDown::root();
        assert!(!navigation.enter_season(false));
        assert_eq!(navigation, LibraryDrillDown::root());

        assert!(navigation.enter_season(true));
        let season = navigation;
        assert!(!navigation.enter_dubbing(false));
        assert_eq!(navigation, season);

        assert!(navigation.enter_dubbing(true));
        let dubbing = navigation;
        assert!(!navigation.enter_episode(false));
        assert_eq!(navigation, dubbing);
    }
}
