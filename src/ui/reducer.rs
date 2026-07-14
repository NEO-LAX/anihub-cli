#![allow(dead_code)]

//! Pure UI state transitions for the redesign.
//!
//! This module deliberately contains no HTTP, storage, process, or terminal
//! operations.  The compatibility `AppState` can mirror this state while an
//! integrator executes the returned effects.

use crate::api::{self, AnimeDetails, AnimeItem, EpisodeSourcesResponse};
use crate::storage::{AppHistory, WatchProgress};
use crossterm::event::{KeyCode, KeyModifiers};
use std::collections::HashMap;

/// Small local primitive kept replaceable by the resource worker's id type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

/// Small local primitive kept replaceable by the resource worker's generation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewGeneration(pub u64);

impl ViewGeneration {
    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Explicit resource state.  Compatibility boolean loading flags should be
/// derived from this state, never used as the source of truth.
#[derive(Clone, Debug, Default)]
pub enum LoadState<T> {
    #[default]
    Idle,
    Loading {
        request_id: RequestId,
        generation: ViewGeneration,
    },
    Ready(T),
    Empty,
    Failed(String),
}

impl<T> LoadState<T> {
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }

    fn accepts(&self, request_id: RequestId, generation: ViewGeneration) -> bool {
        matches!(
            self,
            Self::Loading {
                request_id: expected_request,
                generation: expected_generation,
            } if *expected_request == request_id && *expected_generation == generation
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMode {
    Normal,
    SearchInput,
    Library,
    LibrarySeason,
    LibraryDubbing,
    LibraryEpisode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum FocusPanel {
    SearchList,
    SeasonList,
    DubbingList,
    EpisodeList,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyInput {
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub const fn plain(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }

    pub const fn ctrl_c() -> Self {
        Self::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn is_plain(&self) -> bool {
        !self
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContinueTarget {
    Latest,
    Group {
        anime_ids: Vec<u32>,
        in_library: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageMutation {
    ToggleBookmark {
        anime_id: u32,
    },
    DeleteAnimeProgresses {
        anime_ids: Vec<u32>,
    },
    SetEpisodesWatched {
        anime_id: u32,
        title: String,
        season: u32,
        episodes: Vec<(String, u32)>,
        watched: bool,
    },
    SetEpisodeWatched {
        anime_id: u32,
        title: String,
        season: u32,
        episode: u32,
        studio_name: String,
        watched: bool,
    },
    ResetEpisodeProgress {
        anime_id: u32,
        season: u32,
        episode: u32,
        studio_name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEffect {
    Search {
        request_id: RequestId,
        generation: ViewGeneration,
        query: String,
    },
    Details {
        request_id: RequestId,
        generation: ViewGeneration,
        anime_id: u32,
    },
    CombinedSources {
        request_id: RequestId,
        generation: ViewGeneration,
        representative_id: u32,
        anime_ids: Vec<u32>,
    },
    Poster {
        request_id: RequestId,
        generation: ViewGeneration,
        anime_id: u32,
    },
    Play {
        generation: ViewGeneration,
    },
    Continue {
        generation: ViewGeneration,
        target: ContinueTarget,
    },
    StorageMutation {
        generation: ViewGeneration,
        mutation: StorageMutation,
    },
    OpenBrowser {
        generation: ViewGeneration,
        anime_id: u32,
    },
    Quit,
}

#[derive(Clone, Debug)]
pub enum AppAction {
    Key(KeyInput),
    SearchSubmitted {
        query: String,
    },
    SearchResolved {
        request_id: RequestId,
        generation: ViewGeneration,
        result: Result<Vec<AnimeItem>, String>,
    },
    DetailsResolved {
        request_id: RequestId,
        generation: ViewGeneration,
        anime_id: u32,
        result: Box<Result<AnimeDetails, String>>,
    },
    CombinedSourcesResolved {
        request_id: RequestId,
        generation: ViewGeneration,
        representative_id: u32,
        result: Result<(EpisodeSourcesResponse, Vec<u32>), String>,
    },
    PosterResolved {
        request_id: RequestId,
        generation: ViewGeneration,
        anime_id: u32,
        result: Result<(), String>,
    },
    SelectGroup(usize),
    SelectLibraryItem(usize),
    ResetHome,
    CancelView,
}

#[derive(Clone, Debug)]
pub struct NavigationState {
    pub mode: AppMode,
    pub focus: FocusPanel,
    pub selected_group_index: Option<usize>,
    pub selected_result_index: Option<usize>,
    pub selected_season_index: Option<usize>,
    pub selected_dubbing_index: Option<usize>,
    pub selected_episode_index: Option<usize>,
    pub generation: ViewGeneration,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self {
            mode: AppMode::SearchInput,
            focus: FocusPanel::SearchList,
            selected_group_index: None,
            selected_result_index: None,
            selected_season_index: None,
            selected_dubbing_index: None,
            selected_episode_index: None,
            generation: ViewGeneration::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SearchState {
    pub query: String,
    pub results: Vec<AnimeItem>,
    pub franchise_groups: Vec<Vec<usize>>,
    pub load: LoadState<Vec<AnimeItem>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LibraryIdentity {
    Anime(u32),
    Franchise(u32),
    NormalizedTitle(String),
}

#[derive(Clone, Debug)]
pub struct LibraryAnimeEntry {
    pub identity: LibraryIdentity,
    pub anime_ids: Vec<u32>,
    pub anime_title: String,
    pub latest_progress: WatchProgress,
    pub seasons: Vec<LibrarySeasonEntry>,
}

#[derive(Clone, Debug)]
pub struct LibrarySeasonEntry {
    pub anime_id: u32,
    pub season: u32,
    pub episodes: Vec<WatchProgress>,
}

#[derive(Clone, Debug, Default)]
pub struct LibraryState {
    pub items: Vec<LibraryAnimeEntry>,
    pub selected_index: Option<usize>,
    pub pending_delete_confirmation: Option<(Vec<u32>, String)>,
    pub load: LoadState<Vec<LibraryAnimeEntry>>,
}

#[derive(Clone, Debug, Default)]
pub struct ResourceViewState {
    pub details: LoadState<AnimeDetails>,
    pub sources: LoadState<EpisodeSourcesResponse>,
    pub studio_anime_ids: Vec<u32>,
    pub poster: LoadState<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct PlaybackViewState {
    pub is_playing: bool,
}

#[derive(Clone, Debug, Default)]
pub struct StatusState {
    pub message: Option<String>,
    pub error: bool,
}

/// The reducer-owned model.  `AppState` may expose a compatibility facade,
/// but transitions and stale-response validation happen here.
#[derive(Clone, Debug, Default)]
pub struct UiState {
    pub navigation: NavigationState,
    pub search: SearchState,
    pub library: LibraryState,
    pub resource_view: ResourceViewState,
    pub playback_view: PlaybackViewState,
    pub status: StatusState,
    pub history: AppHistory,
    next_request_id: u64,
}

impl UiState {
    fn request_id(&mut self) -> RequestId {
        self.next_request_id = self.next_request_id.saturating_add(1);
        RequestId(self.next_request_id)
    }

    fn bump_generation(&mut self) -> ViewGeneration {
        let generation = self.navigation.generation.next();
        self.navigation.generation = generation;
        generation
    }

    fn clear_downstream(&mut self) {
        self.navigation.selected_season_index = None;
        self.navigation.selected_dubbing_index = None;
        self.navigation.selected_episode_index = None;
        self.resource_view.details = LoadState::Idle;
        self.resource_view.sources = LoadState::Idle;
        self.resource_view.poster = LoadState::Idle;
        self.resource_view.studio_anime_ids.clear();
    }

    fn start_resources(&mut self, representative_id: u32, anime_ids: Vec<u32>) -> Vec<AppEffect> {
        let generation = self.navigation.generation;
        let details_request = self.request_id();
        let sources_request = self.request_id();
        self.resource_view.details = LoadState::Loading {
            request_id: details_request,
            generation,
        };
        self.resource_view.sources = LoadState::Loading {
            request_id: sources_request,
            generation,
        };
        vec![
            AppEffect::Details {
                request_id: details_request,
                generation,
                anime_id: representative_id,
            },
            AppEffect::CombinedSources {
                request_id: sources_request,
                generation,
                representative_id,
                anime_ids,
            },
        ]
    }

    fn selected_group_ids(&self, index: usize) -> Option<Vec<u32>> {
        let group = self.search.franchise_groups.get(index)?;
        let ids = group
            .iter()
            .filter_map(|&item_index| self.search.results.get(item_index))
            .map(|item| item.id)
            .collect::<Vec<_>>();
        (!ids.is_empty()).then_some(ids)
    }

    fn select_group(&mut self, index: usize) -> Vec<AppEffect> {
        if index >= self.search.franchise_groups.len()
            || self.navigation.selected_group_index == Some(index)
        {
            return Vec::new();
        }
        let Some(anime_ids) = self.selected_group_ids(index) else {
            return Vec::new();
        };
        let representative_index =
            api::representative_idx(&self.search.results, &self.search.franchise_groups[index]);
        let representative_id = self.search.results[representative_index].id;
        self.bump_generation();
        self.navigation.selected_group_index = Some(index);
        self.navigation.selected_result_index = Some(representative_index);
        self.navigation.focus = FocusPanel::SearchList;
        self.clear_downstream();
        self.start_resources(representative_id, anime_ids)
    }

    fn select_library_item(&mut self, index: usize) -> Vec<AppEffect> {
        let Some(item) = self.library.items.get(index) else {
            return Vec::new();
        };
        if self.library.selected_index == Some(index) {
            return Vec::new();
        }
        let representative_id = item.latest_progress.anime_id;
        let anime_ids = item.anime_ids.clone();
        self.bump_generation();
        self.library.selected_index = Some(index);
        self.navigation.mode = AppMode::Library;
        self.navigation.focus = FocusPanel::SearchList;
        self.clear_downstream();
        self.start_resources(representative_id, anime_ids)
    }

    fn search_submitted(&mut self, query: String) -> Vec<AppEffect> {
        let query = query.trim().to_string();
        self.bump_generation();
        self.navigation.mode = AppMode::Normal;
        self.navigation.focus = FocusPanel::SearchList;
        self.search.query = query.clone();
        self.search.results.clear();
        self.search.franchise_groups.clear();
        self.navigation.selected_group_index = None;
        self.navigation.selected_result_index = None;
        self.clear_downstream();
        if query.is_empty() {
            self.search.load = LoadState::Idle;
            return Vec::new();
        }
        let request_id = self.request_id();
        let generation = self.navigation.generation;
        self.search.load = LoadState::Loading {
            request_id,
            generation,
        };
        vec![AppEffect::Search {
            request_id,
            generation,
            query,
        }]
    }

    fn continue_target(&self) -> ContinueTarget {
        if matches!(
            self.navigation.mode,
            AppMode::Library
                | AppMode::LibrarySeason
                | AppMode::LibraryDubbing
                | AppMode::LibraryEpisode
        ) {
            return self
                .library
                .selected_index
                .and_then(|index| self.library.items.get(index))
                .map(|item| ContinueTarget::Group {
                    anime_ids: item.anime_ids.clone(),
                    in_library: true,
                })
                .unwrap_or(ContinueTarget::Latest);
        }
        self.navigation
            .selected_group_index
            .and_then(|index| self.selected_group_ids(index))
            .map(|anime_ids| ContinueTarget::Group {
                anime_ids,
                in_library: false,
            })
            .unwrap_or(ContinueTarget::Latest)
    }

    fn context_anime_id(&self) -> Option<u32> {
        if matches!(
            self.navigation.mode,
            AppMode::Library
                | AppMode::LibrarySeason
                | AppMode::LibraryDubbing
                | AppMode::LibraryEpisode
        ) {
            return self
                .library
                .selected_index
                .and_then(|index| self.library.items.get(index))
                .and_then(|item| item.anime_ids.first().copied());
        }
        self.navigation
            .selected_result_index
            .and_then(|index| self.search.results.get(index))
            .map(|item| item.id)
    }
}

/// Apply one UI action and return work for the integration layer.
pub fn reduce(state: &mut UiState, action: AppAction) -> Vec<AppEffect> {
    match action {
        AppAction::Key(key) => reduce_key(state, key),
        AppAction::SearchSubmitted { query } => state.search_submitted(query),
        AppAction::SelectGroup(index) => state.select_group(index),
        AppAction::SelectLibraryItem(index) => state.select_library_item(index),
        AppAction::ResetHome => {
            let generation = state.bump_generation();
            state.navigation = NavigationState::default();
            state.navigation.generation = generation;
            state.search = SearchState::default();
            state.library = LibraryState::default();
            state.resource_view = ResourceViewState::default();
            state.playback_view = PlaybackViewState::default();
            state.status = StatusState::default();
            Vec::new()
        }
        AppAction::CancelView => {
            state.bump_generation();
            state.resource_view.details = LoadState::Idle;
            state.resource_view.sources = LoadState::Idle;
            state.resource_view.poster = LoadState::Idle;
            Vec::new()
        }
        AppAction::SearchResolved {
            request_id,
            generation,
            result,
        } => {
            if !state.search.load.accepts(request_id, generation)
                || generation != state.navigation.generation
            {
                return Vec::new();
            }
            match result {
                Ok(results) if results.is_empty() => {
                    state.search.results.clear();
                    state.search.franchise_groups.clear();
                    state.search.load = LoadState::Empty;
                    state.status.message = Some("Nothing found".to_string());
                    Vec::new()
                }
                Ok(results) => {
                    let results = api::deduplicate_anime(results);
                    state.search.franchise_groups = api::group_into_franchises(&results);
                    state.search.results = results.clone();
                    state.search.load = if results.is_empty() {
                        LoadState::Empty
                    } else {
                        LoadState::Ready(results)
                    };
                    state.navigation.selected_group_index = None;
                    state.navigation.selected_result_index = None;
                    if state.search.franchise_groups.is_empty() {
                        Vec::new()
                    } else {
                        state.select_group(0)
                    }
                }
                Err(error) => {
                    state.search.load = LoadState::Failed(error);
                    Vec::new()
                }
            }
        }
        AppAction::DetailsResolved {
            request_id,
            generation,
            anime_id,
            result,
        } => {
            if generation != state.navigation.generation
                || !state.resource_view.details.accepts(request_id, generation)
            {
                return Vec::new();
            }
            state.resource_view.details = match *result {
                Ok(details) if details.id == anime_id => LoadState::Ready(details),
                Ok(_) => LoadState::Failed("Details belonged to another anime".to_string()),
                Err(error) => LoadState::Failed(error),
            };
            Vec::new()
        }
        AppAction::CombinedSourcesResolved {
            request_id,
            generation,
            representative_id: _,
            result,
        } => {
            if generation != state.navigation.generation
                || !state.resource_view.sources.accepts(request_id, generation)
            {
                return Vec::new();
            }
            match result {
                Ok((sources, ids)) if sources.ashdi.is_empty() => {
                    state.resource_view.sources = LoadState::Empty;
                    state.resource_view.studio_anime_ids = ids;
                }
                Ok((sources, ids)) => {
                    state.resource_view.sources = LoadState::Ready(sources);
                    state.resource_view.studio_anime_ids = ids;
                }
                Err(error) => state.resource_view.sources = LoadState::Failed(error),
            }
            Vec::new()
        }
        AppAction::PosterResolved {
            request_id,
            generation,
            anime_id,
            result,
        } => {
            if generation != state.navigation.generation
                || !state.resource_view.poster.accepts(request_id, generation)
            {
                return Vec::new();
            }
            state.resource_view.poster = match result {
                Ok(()) => LoadState::Ready(anime_id),
                Err(error) => LoadState::Failed(error),
            };
            Vec::new()
        }
    }
}

fn reduce_key(state: &mut UiState, key: KeyInput) -> Vec<AppEffect> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return vec![AppEffect::Quit];
    }

    match state.navigation.mode {
        AppMode::SearchInput => match key.code {
            KeyCode::Enter => state.search_submitted(state.search.query.clone()),
            KeyCode::Esc => {
                state.navigation.mode = AppMode::Normal;
                Vec::new()
            }
            KeyCode::Backspace => {
                if key.is_plain() {
                    state.search.query.pop();
                }
                Vec::new()
            }
            KeyCode::Char(character) if key.is_plain() => {
                state.search.query.push(character);
                Vec::new()
            }
            _ => Vec::new(),
        },
        AppMode::Normal => reduce_normal_key(state, key),
        AppMode::Library
        | AppMode::LibrarySeason
        | AppMode::LibraryDubbing
        | AppMode::LibraryEpisode => reduce_library_key(state, key),
    }
}

fn reduce_normal_key(state: &mut UiState, key: KeyInput) -> Vec<AppEffect> {
    if !key.is_plain() {
        return Vec::new();
    }
    match key.code {
        KeyCode::Char('q') => vec![AppEffect::Quit],
        KeyCode::Char('c') => vec![AppEffect::Continue {
            generation: state.navigation.generation,
            target: state.continue_target(),
        }],
        KeyCode::Char('b') => state
            .context_anime_id()
            .map(|anime_id| {
                vec![AppEffect::StorageMutation {
                    generation: state.navigation.generation,
                    mutation: StorageMutation::ToggleBookmark { anime_id },
                }]
            })
            .unwrap_or_default(),
        KeyCode::Char('o') => state
            .context_anime_id()
            .map(|anime_id| {
                vec![AppEffect::OpenBrowser {
                    generation: state.navigation.generation,
                    anime_id,
                }]
            })
            .unwrap_or_default(),
        KeyCode::Char('/') => {
            state.navigation.mode = AppMode::SearchInput;
            state.search.query.clear();
            Vec::new()
        }
        KeyCode::Down => move_group(state, true),
        KeyCode::Up => move_group(state, false),
        KeyCode::Right | KeyCode::Enter => move_focus_right(state),
        KeyCode::Left => move_focus_left(state),
        KeyCode::Esc => cancel_focus(state),
        _ => Vec::new(),
    }
}

fn reduce_library_key(state: &mut UiState, key: KeyInput) -> Vec<AppEffect> {
    if !key.is_plain() {
        return Vec::new();
    }
    match key.code {
        KeyCode::Char('q') => vec![AppEffect::Quit],
        KeyCode::Char('c') => vec![AppEffect::Continue {
            generation: state.navigation.generation,
            target: state.continue_target(),
        }],
        KeyCode::Char('o') => state
            .context_anime_id()
            .map(|anime_id| {
                vec![AppEffect::OpenBrowser {
                    generation: state.navigation.generation,
                    anime_id,
                }]
            })
            .unwrap_or_default(),
        KeyCode::Char('/') => {
            state.bump_generation();
            state.navigation = NavigationState {
                generation: state.navigation.generation,
                ..NavigationState::default()
            };
            state.search.query.clear();
            Vec::new()
        }
        KeyCode::Esc | KeyCode::Left => {
            match state.navigation.mode {
                AppMode::LibraryEpisode => state.navigation.mode = AppMode::LibraryDubbing,
                AppMode::LibraryDubbing => state.navigation.mode = AppMode::LibrarySeason,
                AppMode::LibrarySeason => state.navigation.mode = AppMode::Library,
                AppMode::Library => return reduce(state, AppAction::ResetHome),
                _ => {}
            }
            state.bump_generation();
            Vec::new()
        }
        KeyCode::Up | KeyCode::Down => move_library(state, matches!(key.code, KeyCode::Down)),
        KeyCode::Right | KeyCode::Enter => enter_library(state),
        _ => Vec::new(),
    }
}

fn move_group(state: &mut UiState, down: bool) -> Vec<AppEffect> {
    let total = state.search.franchise_groups.len();
    if total == 0 {
        return Vec::new();
    }
    let current = state.navigation.selected_group_index.unwrap_or(0);
    let next = if down {
        (current + 1) % total
    } else if current == 0 {
        total - 1
    } else {
        current - 1
    };
    state.select_group(next)
}

fn move_focus_right(state: &mut UiState) -> Vec<AppEffect> {
    state.navigation.focus = match state.navigation.focus {
        FocusPanel::SearchList => FocusPanel::SeasonList,
        FocusPanel::SeasonList => FocusPanel::DubbingList,
        FocusPanel::DubbingList => FocusPanel::EpisodeList,
        FocusPanel::EpisodeList => {
            return vec![AppEffect::Play {
                generation: state.navigation.generation,
            }];
        }
    };
    Vec::new()
}

fn move_focus_left(state: &mut UiState) -> Vec<AppEffect> {
    state.navigation.focus = match state.navigation.focus {
        FocusPanel::SearchList => return Vec::new(),
        FocusPanel::SeasonList => FocusPanel::SearchList,
        FocusPanel::DubbingList => FocusPanel::SeasonList,
        FocusPanel::EpisodeList => FocusPanel::DubbingList,
    };
    Vec::new()
}

fn cancel_focus(state: &mut UiState) -> Vec<AppEffect> {
    if state.navigation.focus != FocusPanel::SearchList {
        state.navigation.focus = match state.navigation.focus {
            FocusPanel::EpisodeList => FocusPanel::DubbingList,
            FocusPanel::DubbingList => FocusPanel::SeasonList,
            FocusPanel::SeasonList => FocusPanel::SearchList,
            FocusPanel::SearchList => FocusPanel::SearchList,
        };
        state.bump_generation();
    }
    Vec::new()
}

fn enter_library(state: &mut UiState) -> Vec<AppEffect> {
    state.navigation.mode = match state.navigation.mode {
        AppMode::Library => AppMode::LibrarySeason,
        AppMode::LibrarySeason => AppMode::LibraryDubbing,
        AppMode::LibraryDubbing => AppMode::LibraryEpisode,
        AppMode::LibraryEpisode => {
            return vec![AppEffect::Play {
                generation: state.navigation.generation,
            }];
        }
        mode => mode,
    };
    Vec::new()
}

fn move_library(state: &mut UiState, down: bool) -> Vec<AppEffect> {
    if state.navigation.mode == AppMode::Library {
        let total = state.library.items.len();
        if total == 0 {
            return Vec::new();
        }
        let current = state.library.selected_index.unwrap_or(0);
        let next = if down {
            (current + 1) % total
        } else if current == 0 {
            total - 1
        } else {
            current - 1
        };
        return state.select_library_item(next);
    }
    Vec::new()
}

/// Build library entries using IDs first.  `known_groups` supplies explicit
/// franchise membership; only records with no ID use normalized title.
pub fn build_library_items(
    history: &AppHistory,
    known_groups: &[Vec<u32>],
) -> Vec<LibraryAnimeEntry> {
    let mut known_id_to_franchise = HashMap::new();
    for group in known_groups {
        let mut ids = group
            .iter()
            .copied()
            .filter(|id| *id != 0)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        if let Some(&canonical) = ids.first() {
            for id in ids {
                known_id_to_franchise.insert(id, canonical);
            }
        }
    }

    let mut grouped: HashMap<LibraryIdentity, Vec<WatchProgress>> = HashMap::new();
    for progress in history.progress.values() {
        let identity = if let Some(&franchise_id) = known_id_to_franchise.get(&progress.anime_id) {
            LibraryIdentity::Franchise(franchise_id)
        } else if progress.anime_id != 0 {
            LibraryIdentity::Anime(progress.anime_id)
        } else {
            LibraryIdentity::NormalizedTitle(normalize_title(&progress.anime_title))
        };
        grouped.entry(identity).or_default().push(progress.clone());
    }

    let mut items = grouped
        .into_iter()
        .filter_map(|(identity, mut progress)| {
            progress.sort_by_key(|entry| {
                (
                    entry.updated_at,
                    entry.anime_id,
                    entry.season,
                    entry.episode,
                )
            });
            let latest_progress = progress.last()?.clone();
            let anime_title = latest_progress.anime_title.clone();
            let mut season_map: HashMap<u32, Vec<WatchProgress>> = HashMap::new();
            for entry in progress {
                season_map.entry(entry.season).or_default().push(entry);
            }
            let mut seasons = season_map
                .into_iter()
                .map(|(season, mut episodes)| {
                    episodes.sort_by_key(|entry| (entry.episode, entry.anime_id, entry.updated_at));
                    let anime_id = episodes
                        .iter()
                        .max_by_key(|entry| entry.updated_at)
                        .map(|entry| entry.anime_id)
                        .unwrap_or(latest_progress.anime_id);
                    LibrarySeasonEntry {
                        anime_id,
                        season,
                        episodes,
                    }
                })
                .collect::<Vec<_>>();
            seasons.sort_by_key(|entry| entry.season);
            let mut anime_ids = seasons
                .iter()
                .flat_map(|season| season.episodes.iter().map(|entry| entry.anime_id))
                .filter(|id| *id != 0)
                .collect::<Vec<_>>();
            anime_ids.sort_unstable();
            anime_ids.dedup();
            Some(LibraryAnimeEntry {
                identity,
                anime_ids,
                anime_title,
                latest_progress,
                seasons,
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .latest_progress
            .updated_at
            .cmp(&left.latest_progress.updated_at)
            .then_with(|| left.identity.cmp(&right.identity))
    });
    items
}

pub fn normalize_title(title: &str) -> String {
    title
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u32, title: &str) -> AnimeItem {
        AnimeItem {
            id,
            anilist_id: None,
            slug: id.to_string(),
            title_ukrainian: title.to_string(),
            title_original: None,
            title_english: None,
            status: String::new(),
            anime_type: "TV".to_string(),
            year: Some(id),
            has_ukrainian_dub: true,
        }
    }

    fn progress(id: u32, title: &str, updated_at: i64) -> WatchProgress {
        WatchProgress {
            anime_id: id,
            anime_title: title.to_string(),
            season: 1,
            episode: 1,
            studio_name: "dub".to_string(),
            timestamp: 0.0,
            duration: 0.0,
            watched: false,
            updated_at,
        }
    }

    fn history(entries: Vec<WatchProgress>) -> AppHistory {
        AppHistory {
            progress: entries
                .into_iter()
                .enumerate()
                .map(|(index, entry)| (index.to_string(), entry))
                .collect(),
            bookmarks: Vec::new(),
        }
    }

    fn selected_state() -> UiState {
        let mut state = UiState::default();
        state.navigation.mode = AppMode::Normal;
        state.search.results = vec![item(1, "A"), item(2, "B")];
        state.search.franchise_groups = vec![vec![0], vec![1]];
        state
    }

    #[test]
    fn stale_generation_is_ignored() {
        let mut state = UiState::default();
        let first = reduce(&mut state, AppAction::SearchSubmitted { query: "a".into() });
        let (first_request, first_generation) = match &first[0] {
            AppEffect::Search {
                request_id,
                generation,
                ..
            } => (*request_id, *generation),
            effect => panic!("unexpected effect: {effect:?}"),
        };
        let second = reduce(&mut state, AppAction::SearchSubmitted { query: "b".into() });
        let (second_request, second_generation) = match &second[0] {
            AppEffect::Search {
                request_id,
                generation,
                ..
            } => (*request_id, *generation),
            effect => panic!("unexpected effect: {effect:?}"),
        };
        reduce(
            &mut state,
            AppAction::SearchResolved {
                request_id: first_request,
                generation: first_generation,
                result: Ok(vec![item(1, "old")]),
            },
        );
        assert!(state.search.results.is_empty());
        reduce(
            &mut state,
            AppAction::SearchResolved {
                request_id: second_request,
                generation: second_generation,
                result: Ok(vec![item(2, "new")]),
            },
        );
        assert_eq!(state.search.results[0].id, 2);
    }

    #[test]
    fn selection_a_then_b_rejects_a_details_result() {
        let mut state = selected_state();
        let a = reduce(&mut state, AppAction::SelectGroup(0));
        let (a_request, a_generation) = match &a[0] {
            AppEffect::Details {
                request_id,
                generation,
                ..
            } => (*request_id, *generation),
            effect => panic!("unexpected effect: {effect:?}"),
        };
        let b = reduce(&mut state, AppAction::SelectGroup(1));
        let (b_request, b_generation) = match &b[0] {
            AppEffect::Details {
                request_id,
                generation,
                ..
            } => (*request_id, *generation),
            effect => panic!("unexpected effect: {effect:?}"),
        };
        reduce(
            &mut state,
            AppAction::DetailsResolved {
                request_id: a_request,
                generation: a_generation,
                anime_id: 1,
                result: Box::new(Err("stale".into())),
            },
        );
        assert!(state.resource_view.details.is_loading());
        let details = AnimeDetails {
            id: 2,
            anilist_id: None,
            slug: "b".into(),
            title_ukrainian: "B".into(),
            title_original: None,
            title_english: None,
            status: String::new(),
            anime_type: "TV".into(),
            year: None,
            has_ukrainian_dub: true,
            poster_url: None,
            episodes_count: None,
            description: None,
            rating: None,
            genres: None,
            dubbing_studios: None,
        };
        reduce(
            &mut state,
            AppAction::DetailsResolved {
                request_id: b_request,
                generation: b_generation,
                anime_id: 2,
                result: Box::new(Ok(details)),
            },
        );
        assert_eq!(
            state
                .resource_view
                .details
                .ready()
                .map(|details| details.id),
            Some(2)
        );
    }

    #[test]
    fn reset_cancels_old_view() {
        let mut state = selected_state();
        let effects = reduce(&mut state, AppAction::SelectGroup(0));
        let (request_id, generation) = match effects[1] {
            AppEffect::CombinedSources {
                request_id,
                generation,
                ..
            } => (request_id, generation),
            _ => panic!("missing sources effect"),
        };
        reduce(&mut state, AppAction::ResetHome);
        reduce(
            &mut state,
            AppAction::CombinedSourcesResolved {
                request_id,
                generation,
                representative_id: 1,
                result: Err("stale".into()),
            },
        );
        assert!(matches!(state.resource_view.sources, LoadState::Idle));
        assert_eq!(state.navigation.mode, AppMode::SearchInput);
    }

    #[test]
    fn ctrl_c_quits_but_plain_c_continues() {
        let mut state = selected_state();
        assert_eq!(
            reduce(&mut state, AppAction::Key(KeyInput::ctrl_c())),
            vec![AppEffect::Quit]
        );
        let effects = reduce(
            &mut state,
            AppAction::Key(KeyInput::plain(KeyCode::Char('c'))),
        );
        assert!(matches!(effects.as_slice(), [AppEffect::Continue { .. }]));
    }

    #[test]
    fn library_identity_uses_ids_then_known_franchise_ids_then_title_fallback() {
        let same_title = build_library_items(
            &history(vec![
                progress(1, "Same title", 1),
                progress(2, "Same title", 2),
            ]),
            &[],
        );
        assert_eq!(same_title.len(), 2);

        let known_group = build_library_items(
            &history(vec![progress(1, "S1", 1), progress(2, "S2", 2)]),
            &[vec![2, 1]],
        );
        assert_eq!(known_group.len(), 1);
        assert_eq!(known_group[0].anime_ids, vec![1, 2]);

        let title_fallback = build_library_items(
            &history(vec![
                progress(0, "  Same   TITLE! ", 1),
                progress(0, "same title", 2),
            ]),
            &[],
        );
        assert_eq!(title_fallback.len(), 1);
    }
}
