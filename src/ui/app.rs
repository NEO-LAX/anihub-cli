use crate::api::anilist::FranchiseMember;
use crate::api::{self, AnimeDetails, AnimeItem, ApiClient, AshdiStudio, EpisodeSourcesResponse};
use crate::storage::{AppHistory, StorageManager, WatchProgress};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use image::DynamicImage;
use ratatui::widgets::ListState;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, PartialEq)]
pub enum AppMode {
    Normal,
    SearchInput,
    Library,
    LibrarySeason,
    LibraryDubbing,
    LibraryEpisode,
}

#[derive(PartialEq)]
pub enum FocusPanel {
    SearchList,
    SeasonList,
    DubbingList,
    EpisodeList,
}

#[derive(Clone, PartialEq)]
pub enum StatusKind {
    Info,
    Error,
}

#[derive(Clone)]
pub enum ContinueRequest {
    Latest,
    Group {
        anime_ids: Vec<u32>,
        in_library: bool,
    },
}

#[derive(Clone)]
pub struct LibraryAnimeEntry {
    pub anime_ids: Vec<u32>,
    pub anime_title: String,
    pub latest_progress: WatchProgress,
    pub seasons: Vec<LibrarySeasonEntry>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct LibrarySeasonEntry {
    pub anime_id: u32,
    pub season: u32,
    pub episodes: Vec<WatchProgress>,
}

pub struct AppState {
    pub mode: AppMode,
    pub focus: FocusPanel,
    pub search_query: String,

    pub search_results: Vec<AnimeItem>,
    pub franchise_groups: Vec<Vec<usize>>,
    pub selected_group_index: Option<usize>,
    pub selected_result_index: Option<usize>,
    pub selected_season_index: Option<usize>,
    pub selected_dubbing_index: Option<usize>,
    pub selected_episode_index: Option<usize>,

    pub current_sources: Option<EpisodeSourcesResponse>,
    pub current_details: Option<AnimeDetails>,

    // Для кожного studio entry в current_sources.ashdi — який anime_id він представляє
    pub studio_anime_ids: Vec<u32>,
    // Який search_result показувати в сайдбарі (None = selected_result_index)
    pub sidebar_anime_idx: Option<usize>,

    pub result_list_state: ListState,
    pub season_list_state: ListState,
    pub dubbing_list_state: ListState,
    pub episode_list_state: ListState,

    pub library_items: Vec<LibraryAnimeEntry>,
    pub library_anime_index: Option<usize>,
    pub library_season_index: Option<usize>,
    pub library_episode_index: Option<usize>,
    pub library_anime_list_state: ListState,
    pub library_season_list_state: ListState,
    pub library_episode_list_state: ListState,
    pub pending_delete_confirmation: Option<(Vec<u32>, String)>,

    pub should_quit: bool,
    pub api_client: ApiClient,
    pub storage: StorageManager,
    pub history: AppHistory,
    pub loading: bool,
    pub play_episode: bool,
    pub continue_request: Option<ContinueRequest>,
    pub status_message: Option<(String, StatusKind)>,
    pub status_expires_at: Option<Instant>,

    // Стан відтворення
    pub is_playing: bool,
    pub mpv_player: crate::player::MpvPlayer,
    pub mpv_child: Option<Child>,
    pub mpv_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::player::MpvEvent>>,
    pub mpv_monitor: Option<JoinHandle<()>>,
    pub pending_progress: Option<(u32, String, u32, u32, String)>,
    pub mpv_playlist: Vec<(u32, String, u32, u32, String)>,
    pub mpv_last_time: f64,
    pub mpv_last_duration: f64,

    // Кеші та prefetch
    pub search_cache: HashMap<String, Vec<AnimeItem>>,
    pub details_cache: HashMap<u32, AnimeDetails>,
    pub sources_cache: HashMap<u32, EpisodeSourcesResponse>,
    pub prefetching: bool,
    pub prefetch_rx:
        Option<mpsc::Receiver<(u32, Option<AnimeDetails>, Option<EpisodeSourcesResponse>)>>,
    pub preload_abort: Option<AbortHandle>,
    pub pending_prefetch_ids: Option<Vec<u32>>,

    // Обкладинка
    pub picker: Picker,
    pub current_poster: Option<StatefulProtocol>,
    pub poster_cache: HashMap<u32, DynamicImage>,
    pub poster_fetch_pending: Option<u32>,

    // AniList — кеш членів франшизи (ключ: representative_id)
    pub anilist_cache: HashMap<u32, Vec<FranchiseMember>>,
}

impl AppState {
    pub fn new(picker: Picker) -> anyhow::Result<Self> {
        let storage = StorageManager::new()?;
        let history = storage.load_history().unwrap_or_default();

        Ok(Self {
            mode: AppMode::SearchInput,
            focus: FocusPanel::SearchList,
            search_query: String::new(),

            search_results: Vec::new(),
            franchise_groups: Vec::new(),
            selected_group_index: None,
            selected_result_index: None,
            selected_season_index: None,
            selected_dubbing_index: None,
            selected_episode_index: None,
            current_sources: None,
            current_details: None,

            studio_anime_ids: Vec::new(),
            sidebar_anime_idx: None,

            result_list_state: ListState::default(),
            season_list_state: ListState::default(),
            dubbing_list_state: ListState::default(),
            episode_list_state: ListState::default(),

            library_items: Vec::new(),
            library_anime_index: None,
            library_season_index: None,
            library_episode_index: None,
            library_anime_list_state: ListState::default(),
            library_season_list_state: ListState::default(),
            library_episode_list_state: ListState::default(),
            pending_delete_confirmation: None,

            should_quit: false,
            api_client: ApiClient::new()?,
            storage,
            history,
            loading: false,
            play_episode: false,
            continue_request: None,
            status_message: None,
            status_expires_at: None,

            is_playing: false,
            mpv_player: crate::player::MpvPlayer::new()?,
            mpv_child: None,
            mpv_rx: None,
            mpv_monitor: None,
            pending_progress: None,
            mpv_playlist: Vec::new(),
            mpv_last_time: 0.0,
            mpv_last_duration: 0.0,

            search_cache: HashMap::new(),
            details_cache: HashMap::new(),
            sources_cache: HashMap::new(),
            prefetching: false,
            prefetch_rx: None,
            preload_abort: None,
            pending_prefetch_ids: None,

            picker,
            current_poster: None,
            poster_cache: HashMap::new(),
            poster_fetch_pending: None,

            anilist_cache: HashMap::new(),
        })
    }

    // --- Хелпери для 4-панельної навігації ---

    /// Унікальні season_number з current_sources, відсортовані за зростанням.
    pub fn unique_seasons(&self) -> Vec<u32> {
        let Some(sources) = &self.current_sources else {
            return Vec::new();
        };
        let mut seasons: Vec<u32> = Vec::new();
        for s in &sources.ashdi {
            if !seasons.contains(&s.season_number) {
                seasons.push(s.season_number);
            }
        }
        seasons.sort();
        seasons
    }

    /// Студії для заданого season_number у порядку як у current_sources.
    pub fn studios_for_season(&self, season_num: u32) -> Vec<&AshdiStudio> {
        let Some(sources) = &self.current_sources else {
            return Vec::new();
        };
        sources
            .ashdi
            .iter()
            .filter(|s| s.season_number == season_num)
            .collect()
    }

    /// Поточний season_number за selected_season_index.
    pub fn selected_season_num(&self) -> Option<u32> {
        let idx = self.selected_season_index?;
        if self.is_library_mode() {
            self.library_season_numbers().get(idx).copied()
        } else {
            self.unique_seasons().get(idx).copied()
        }
    }

    /// Обрана студія (для відтворення та списку епізодів).
    pub fn selected_studio(&self) -> Option<&AshdiStudio> {
        let season_num = self.selected_season_num()?;
        let dub_idx = self.selected_dubbing_index?;
        self.studios_for_season(season_num).into_iter().nth(dub_idx)
    }

    pub fn library_selected_anime(&self) -> Option<&LibraryAnimeEntry> {
        self.library_anime_index
            .and_then(|idx| self.library_items.get(idx))
    }

    pub fn library_season_numbers(&self) -> Vec<u32> {
        if self.current_sources.is_some() {
            return self.unique_seasons();
        }

        let Some(anime) = self.library_selected_anime() else {
            return Vec::new();
        };
        let mut seasons: Vec<u32> = anime.seasons.iter().map(|season| season.season).collect();
        seasons.sort_unstable();
        seasons
    }

    #[allow(dead_code)]
    pub fn library_selected_season(&self) -> Option<&LibrarySeasonEntry> {
        let anime = self.library_selected_anime()?;
        self.library_season_index
            .and_then(|idx| anime.seasons.get(idx))
    }

    #[allow(dead_code)]
    pub fn library_selected_episode(&self) -> Option<&WatchProgress> {
        let season = self.library_selected_season()?;
        self.library_episode_index
            .and_then(|idx| season.episodes.get(idx))
    }

    pub fn is_library_mode(&self) -> bool {
        matches!(
            self.mode,
            AppMode::Library
                | AppMode::LibrarySeason
                | AppMode::LibraryDubbing
                | AppMode::LibraryEpisode
        )
    }

    // ---

    pub fn handle_events(&mut self) -> anyhow::Result<()> {
        self.clear_expired_status();

        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if self.handle_pending_delete_confirmation(key.code) {
                        return Ok(());
                    }
                    self.clear_info_status();
                    match self.mode {
                        AppMode::Normal => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char('c') => self.request_continue(),
                            KeyCode::Char('x') => self.toggle_search_selection_watched(),
                            KeyCode::Char('l') => self.open_library(),
                            KeyCode::Esc => self.handle_esc(),
                            KeyCode::Char('/') => {
                                self.mode = AppMode::SearchInput;
                                self.search_query.clear();
                            }
                            KeyCode::Right => self.move_focus_right(),
                            KeyCode::Left => self.move_focus_left(),
                            KeyCode::Down => self.move_selection_down(),
                            KeyCode::Up => self.move_selection_up(),
                            KeyCode::Enter => self.handle_enter(),
                            _ => {}
                        },
                        AppMode::SearchInput => match key.code {
                            KeyCode::Enter => {
                                self.mode = AppMode::Normal;
                                if !self.search_query.is_empty() {
                                    self.loading = true;
                                }
                            }
                            KeyCode::Char(c) => self.search_query.push(c),
                            KeyCode::Backspace => {
                                self.search_query.pop();
                            }
                            KeyCode::Esc => {
                                self.mode = AppMode::Normal;
                            }
                            _ => {}
                        },
                        AppMode::Library => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char('c') => self.request_continue(),
                            KeyCode::Char('d') => self.delete_library_selection(),
                            KeyCode::Char('/') => self.open_global_search(),
                            KeyCode::Left => {}
                            KeyCode::Esc => self.reset_to_home(),
                            KeyCode::Up => self.move_library_up(),
                            KeyCode::Down => self.move_library_down(),
                            KeyCode::Right | KeyCode::Enter => self.enter_library_season(),
                            _ => {}
                        },
                        AppMode::LibrarySeason => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char('x') => self.toggle_library_selection_watched(),
                            KeyCode::Char('/') => self.open_global_search(),
                            KeyCode::Left => self.leave_library_level(),
                            KeyCode::Esc => self.leave_library_level(),
                            KeyCode::Up => self.move_library_up(),
                            KeyCode::Down => self.move_library_down(),
                            KeyCode::Right | KeyCode::Enter => self.enter_library_dubbing(),
                            _ => {}
                        },
                        AppMode::LibraryDubbing => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char('x') => self.toggle_library_selection_watched(),
                            KeyCode::Char('/') => self.open_global_search(),
                            KeyCode::Left => self.leave_library_level(),
                            KeyCode::Esc => self.leave_library_level(),
                            KeyCode::Up => self.move_library_up(),
                            KeyCode::Down => self.move_library_down(),
                            KeyCode::Right | KeyCode::Enter => self.enter_library_episode(),
                            _ => {}
                        },
                        AppMode::LibraryEpisode => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char('x') => self.toggle_library_selection_watched(),
                            KeyCode::Char('/') => self.open_global_search(),
                            KeyCode::Left => self.leave_library_level(),
                            KeyCode::Esc => self.leave_library_level(),
                            KeyCode::Up => self.move_library_up(),
                            KeyCode::Down => self.move_library_down(),
                            KeyCode::Enter => self.play_episode = true,
                            _ => {}
                        },
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_esc(&mut self) {
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
            FocusPanel::DubbingList => self.focus = FocusPanel::SeasonList,
            FocusPanel::SeasonList => {
                self.focus = FocusPanel::SearchList;
                self.sidebar_anime_idx = None;
                self.restore_representative_poster();
            }
            FocusPanel::SearchList => self.reset_to_home(),
        }
    }

    fn handle_enter(&mut self) {
        if self.focus != FocusPanel::EpisodeList {
            self.move_focus_right();
        } else {
            self.play_episode = true;
        }
    }

    fn move_focus_right(&mut self) {
        self.focus = match self.focus {
            FocusPanel::SearchList => {
                if self.selected_result_index.is_some() {
                    let has_seasons = self
                        .current_sources
                        .as_ref()
                        .map_or(false, |s| !s.ashdi.is_empty());
                    if has_seasons && !self.unique_seasons().is_empty() {
                        self.selected_season_index = Some(0);
                        self.season_list_state.select(Some(0));
                        self.update_sidebar_for_season();
                        FocusPanel::SeasonList
                    } else {
                        FocusPanel::SearchList
                    }
                } else {
                    FocusPanel::SearchList
                }
            }
            FocusPanel::SeasonList => {
                let season_num = self.selected_season_num();
                if let Some(sn) = season_num {
                    let studios_len = self.studios_for_season(sn).len();
                    if studios_len > 0 {
                        self.selected_dubbing_index = Some(0);
                        self.dubbing_list_state.select(Some(0));
                        FocusPanel::DubbingList
                    } else {
                        FocusPanel::SeasonList
                    }
                } else {
                    FocusPanel::SeasonList
                }
            }
            FocusPanel::DubbingList => {
                let has_episodes = self
                    .selected_studio()
                    .map_or(false, |s| !s.episodes.is_empty());
                if has_episodes {
                    self.selected_episode_index = Some(0);
                    self.episode_list_state.select(Some(0));
                    FocusPanel::EpisodeList
                } else {
                    FocusPanel::DubbingList
                }
            }
            FocusPanel::EpisodeList => FocusPanel::EpisodeList,
        };
    }

    fn move_focus_left(&mut self) {
        match self.focus {
            FocusPanel::EpisodeList => self.focus = FocusPanel::DubbingList,
            FocusPanel::DubbingList => self.focus = FocusPanel::SeasonList,
            FocusPanel::SeasonList => {
                self.focus = FocusPanel::SearchList;
                self.sidebar_anime_idx = None;
                self.restore_representative_poster();
            }
            FocusPanel::SearchList => {}
        }
    }

    fn move_selection_down(&mut self) {
        match self.focus {
            FocusPanel::SearchList => {
                let total = self.franchise_groups.len();
                if total == 0 {
                    return;
                }
                let i = match self.result_list_state.selected() {
                    Some(i) => {
                        if i >= total.saturating_sub(1) {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.result_list_state.select(Some(i));
                self.selected_group_index = Some(i);
                if let Some(group) = self.franchise_groups.get(i) {
                    self.selected_result_index =
                        Some(api::representative_idx(&self.search_results, group));
                }
                self.reset_downstream();
            }
            FocusPanel::SeasonList => {
                let total = self.unique_seasons().len();
                if total == 0 {
                    return;
                }
                let i = match self.season_list_state.selected() {
                    Some(i) => {
                        if i >= total.saturating_sub(1) {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.season_list_state.select(Some(i));
                self.selected_season_index = Some(i);
                self.selected_dubbing_index = None;
                self.dubbing_list_state.select(None);
                self.update_sidebar_for_season();
            }
            FocusPanel::DubbingList => {
                if let Some(sn) = self.selected_season_num() {
                    let studios_len = self.studios_for_season(sn).len();
                    if studios_len == 0 {
                        return;
                    }
                    let i = match self.dubbing_list_state.selected() {
                        Some(i) => {
                            if i >= studios_len.saturating_sub(1) {
                                0
                            } else {
                                i + 1
                            }
                        }
                        None => 0,
                    };
                    self.dubbing_list_state.select(Some(i));
                    self.selected_dubbing_index = Some(i);
                }
            }
            FocusPanel::EpisodeList => {
                let episodes_len = self.selected_studio().map_or(0, |s| s.episodes.len());
                if episodes_len == 0 {
                    return;
                }
                let i = match self.episode_list_state.selected() {
                    Some(i) => {
                        if i >= episodes_len.saturating_sub(1) {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.episode_list_state.select(Some(i));
                self.selected_episode_index = Some(i);
            }
        }
    }

    fn move_selection_up(&mut self) {
        match self.focus {
            FocusPanel::SearchList => {
                let total = self.franchise_groups.len();
                if total == 0 {
                    return;
                }
                let i = match self.result_list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            total.saturating_sub(1)
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.result_list_state.select(Some(i));
                self.selected_group_index = Some(i);
                if let Some(group) = self.franchise_groups.get(i) {
                    self.selected_result_index =
                        Some(api::representative_idx(&self.search_results, group));
                }
                self.reset_downstream();
            }
            FocusPanel::SeasonList => {
                let total = self.unique_seasons().len();
                if total == 0 {
                    return;
                }
                let i = match self.season_list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            total.saturating_sub(1)
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.season_list_state.select(Some(i));
                self.selected_season_index = Some(i);
                self.selected_dubbing_index = None;
                self.dubbing_list_state.select(None);
                self.update_sidebar_for_season();
            }
            FocusPanel::DubbingList => {
                if let Some(sn) = self.selected_season_num() {
                    let studios_len = self.studios_for_season(sn).len();
                    if studios_len == 0 {
                        return;
                    }
                    let i = match self.dubbing_list_state.selected() {
                        Some(i) => {
                            if i == 0 {
                                studios_len.saturating_sub(1)
                            } else {
                                i - 1
                            }
                        }
                        None => 0,
                    };
                    self.dubbing_list_state.select(Some(i));
                    self.selected_dubbing_index = Some(i);
                }
            }
            FocusPanel::EpisodeList => {
                let episodes_len = self.selected_studio().map_or(0, |s| s.episodes.len());
                if episodes_len == 0 {
                    return;
                }
                let i = match self.episode_list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            episodes_len.saturating_sub(1)
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.episode_list_state.select(Some(i));
                self.selected_episode_index = Some(i);
            }
        }
    }

    fn reset_downstream(&mut self) {
        self.loading = true;
        self.current_sources = None;
        self.current_details = None;
        self.current_poster = None;
        self.studio_anime_ids.clear();
        self.sidebar_anime_idx = None;
        self.selected_season_index = None;
        self.season_list_state.select(None);
        self.selected_dubbing_index = None;
        self.dubbing_list_state.select(None);
        self.selected_episode_index = None;
        self.episode_list_state.select(None);
    }

    fn reset_to_home(&mut self) {
        if let Some(abort) = self.preload_abort.take() {
            abort.abort();
        }

        self.mode = AppMode::SearchInput;
        self.focus = FocusPanel::SearchList;
        self.search_query.clear();
        self.search_results.clear();
        self.franchise_groups.clear();
        self.selected_group_index = None;
        self.selected_result_index = None;
        self.current_sources = None;
        self.current_details = None;
        self.studio_anime_ids.clear();
        self.sidebar_anime_idx = None;
        self.result_list_state.select(None);
        self.selected_season_index = None;
        self.season_list_state.select(None);
        self.selected_dubbing_index = None;
        self.dubbing_list_state.select(None);
        self.selected_episode_index = None;
        self.episode_list_state.select(None);
        self.loading = false;
        self.prefetching = false;
        self.prefetch_rx = None;
        self.clear_status();
        self.current_poster = None;
        self.poster_fetch_pending = None;
        self.library_items.clear();
        self.library_anime_index = None;
        self.library_season_index = None;
        self.library_episode_index = None;
        self.library_anime_list_state.select(None);
        self.library_season_list_state.select(None);
        self.library_episode_list_state.select(None);
        self.pending_delete_confirmation = None;
    }

    fn open_global_search(&mut self) {
        self.reset_to_home();
        self.mode = AppMode::SearchInput;
    }

    pub fn open_library(&mut self) {
        if self.is_library_mode() {
            return;
        }

        self.library_items = build_library_items(&self.history);
        self.library_anime_index = (!self.library_items.is_empty()).then_some(0);
        self.library_season_index = None;
        self.library_episode_index = None;
        self.library_anime_list_state
            .select(self.library_anime_index);
        self.library_season_list_state.select(None);
        self.library_episode_list_state.select(None);
        self.mode = AppMode::Library;
        self.pending_delete_confirmation = None;
        self.prepare_library_anime_selection();
        self.pending_prefetch_ids = Some(
            self.library_items
                .iter()
                .map(|item| item.latest_progress.anime_id)
                .collect(),
        );

        if self.library_items.is_empty() {
            self.set_info_status("Бібліотека порожня");
        }
    }

    pub fn library_selected_anime_id(&self) -> Option<u32> {
        if let Some(season_num) = self.selected_season_num() {
            if let Some(anime_id) = self.current_sources.as_ref().and_then(|sources| {
                sources
                    .ashdi
                    .iter()
                    .position(|studio| studio.season_number == season_num)
                    .and_then(|idx| self.studio_anime_ids.get(idx))
                    .copied()
            }) {
                return Some(anime_id);
            }

            if let Some(anime) = self.library_selected_anime() {
                if let Some(season) = anime
                    .seasons
                    .iter()
                    .find(|season| season.season == season_num)
                {
                    return Some(season.anime_id);
                }
            }
        }

        self.library_selected_anime().map(|anime| {
            if anime_is_fully_watched(anime) {
                anime
                    .seasons
                    .first()
                    .map(|season| season.anime_id)
                    .unwrap_or(anime.latest_progress.anime_id)
            } else {
                anime.latest_progress.anime_id
            }
        })
    }

    fn prepare_library_anime_selection(&mut self) {
        self.selected_season_index = None;
        self.selected_dubbing_index = None;
        self.selected_episode_index = None;
        self.season_list_state.select(None);
        self.dubbing_list_state.select(None);
        self.episode_list_state.select(None);
        self.current_sources = None;
        self.current_details = None;
        self.current_poster = None;
        self.studio_anime_ids.clear();
        self.sync_library_sidebar_selection();
        self.loading = true;
    }

    fn leave_library_level(&mut self) {
        match self.mode {
            AppMode::LibraryEpisode => {
                self.mode = AppMode::LibraryDubbing;
                self.selected_episode_index = None;
                self.episode_list_state.select(None);
            }
            AppMode::LibraryDubbing => {
                self.mode = AppMode::LibrarySeason;
                self.selected_dubbing_index = None;
                self.dubbing_list_state.select(None);
                self.selected_episode_index = None;
                self.episode_list_state.select(None);
            }
            AppMode::LibrarySeason => {
                self.mode = AppMode::Library;
                self.selected_season_index = None;
                self.selected_dubbing_index = None;
                self.selected_episode_index = None;
                self.season_list_state.select(None);
                self.dubbing_list_state.select(None);
                self.episode_list_state.select(None);
            }
            AppMode::Library => self.reset_to_home(),
            _ => {}
        }
    }

    fn enter_library_season(&mut self) {
        if self
            .library_selected_anime()
            .is_none_or(|anime| anime.seasons.is_empty())
        {
            return;
        }

        self.mode = AppMode::LibrarySeason;
        self.selected_season_index = Some(0);
        self.selected_dubbing_index = None;
        self.selected_episode_index = None;
        self.season_list_state.select(Some(0));
        self.dubbing_list_state.select(None);
        self.episode_list_state.select(None);
        self.sync_library_sidebar_selection();
    }

    fn enter_library_dubbing(&mut self) {
        let Some(season_num) = self.selected_season_num() else {
            return;
        };
        if self.studios_for_season(season_num).is_empty() {
            return;
        }

        self.mode = AppMode::LibraryDubbing;
        self.selected_dubbing_index = Some(0);
        self.selected_episode_index = None;
        self.dubbing_list_state.select(Some(0));
        self.episode_list_state.select(None);
    }

    fn enter_library_episode(&mut self) {
        if self
            .selected_studio()
            .is_none_or(|studio| studio.episodes.is_empty())
        {
            return;
        }
        self.mode = AppMode::LibraryEpisode;
        self.selected_episode_index = Some(0);
        self.episode_list_state.select(Some(0));
    }

    fn move_library_down(&mut self) {
        match self.mode {
            AppMode::Library => {
                let total = self.library_items.len();
                if total == 0 {
                    return;
                }
                let next = match self.library_anime_list_state.selected() {
                    Some(i) if i >= total.saturating_sub(1) => 0,
                    Some(i) => i + 1,
                    None => 0,
                };
                self.library_anime_index = Some(next);
                self.library_anime_list_state.select(Some(next));
                self.prepare_library_anime_selection();
            }
            AppMode::LibrarySeason => {
                let total = self.library_season_numbers().len();
                if total == 0 {
                    return;
                }
                let next = match self.season_list_state.selected() {
                    Some(i) if i >= total.saturating_sub(1) => 0,
                    Some(i) => i + 1,
                    None => 0,
                };
                self.selected_season_index = Some(next);
                self.season_list_state.select(Some(next));
                self.sync_library_sidebar_selection();
            }
            AppMode::LibraryDubbing => {
                let Some(season_num) = self.selected_season_num() else {
                    return;
                };
                let total = self.studios_for_season(season_num).len();
                if total == 0 {
                    return;
                }
                let next = match self.dubbing_list_state.selected() {
                    Some(i) if i >= total.saturating_sub(1) => 0,
                    Some(i) => i + 1,
                    None => 0,
                };
                self.selected_dubbing_index = Some(next);
                self.dubbing_list_state.select(Some(next));
            }
            AppMode::LibraryEpisode => {
                let total = self
                    .selected_studio()
                    .map_or(0, |season| season.episodes.len());
                if total == 0 {
                    return;
                }
                let next = match self.episode_list_state.selected() {
                    Some(i) if i >= total.saturating_sub(1) => 0,
                    Some(i) => i + 1,
                    None => 0,
                };
                self.selected_episode_index = Some(next);
                self.episode_list_state.select(Some(next));
            }
            _ => {}
        }
    }

    fn move_library_up(&mut self) {
        match self.mode {
            AppMode::Library => {
                let total = self.library_items.len();
                if total == 0 {
                    return;
                }
                let next = match self.library_anime_list_state.selected() {
                    Some(0) => total.saturating_sub(1),
                    Some(i) => i - 1,
                    None => 0,
                };
                self.library_anime_index = Some(next);
                self.library_anime_list_state.select(Some(next));
                self.prepare_library_anime_selection();
            }
            AppMode::LibrarySeason => {
                let total = self.library_season_numbers().len();
                if total == 0 {
                    return;
                }
                let next = match self.season_list_state.selected() {
                    Some(0) => total.saturating_sub(1),
                    Some(i) => i - 1,
                    None => 0,
                };
                self.selected_season_index = Some(next);
                self.season_list_state.select(Some(next));
                self.sync_library_sidebar_selection();
            }
            AppMode::LibraryDubbing => {
                let Some(season_num) = self.selected_season_num() else {
                    return;
                };
                let total = self.studios_for_season(season_num).len();
                if total == 0 {
                    return;
                }
                let next = match self.dubbing_list_state.selected() {
                    Some(0) => total.saturating_sub(1),
                    Some(i) => i - 1,
                    None => 0,
                };
                self.selected_dubbing_index = Some(next);
                self.dubbing_list_state.select(Some(next));
            }
            AppMode::LibraryEpisode => {
                let total = self
                    .selected_studio()
                    .map_or(0, |season| season.episodes.len());
                if total == 0 {
                    return;
                }
                let next = match self.episode_list_state.selected() {
                    Some(0) => total.saturating_sub(1),
                    Some(i) => i - 1,
                    None => 0,
                };
                self.selected_episode_index = Some(next);
                self.episode_list_state.select(Some(next));
            }
            _ => {}
        }
    }

    pub fn set_info_status(&mut self, message: impl Into<String>) {
        self.status_message = Some((message.into(), StatusKind::Info));
        self.status_expires_at = Some(Instant::now() + Duration::from_secs(4));
    }

    pub fn set_error_status(&mut self, message: impl Into<String>) {
        self.status_message = Some((message.into(), StatusKind::Error));
        self.status_expires_at = None;
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
        self.status_expires_at = None;
    }

    fn clear_info_status(&mut self) {
        if matches!(self.status_message, Some((_, StatusKind::Info))) {
            self.clear_status();
        }
    }

    fn clear_expired_status(&mut self) {
        if let Some(deadline) = self.status_expires_at {
            if Instant::now() >= deadline {
                self.clear_status();
            }
        }
    }

    fn request_continue(&mut self) {
        self.continue_request = match self.mode {
            AppMode::Library
            | AppMode::LibrarySeason
            | AppMode::LibraryDubbing
            | AppMode::LibraryEpisode => {
                self.library_selected_anime()
                    .map(|anime| ContinueRequest::Group {
                        anime_ids: anime.anime_ids.clone(),
                        in_library: true,
                    })
            }
            _ => self
                .selected_group_index
                .and_then(|g_idx| {
                    if !self.studio_anime_ids.is_empty() {
                        let mut anime_ids = self.studio_anime_ids.clone();
                        anime_ids.sort_unstable();
                        anime_ids.dedup();
                        Some(ContinueRequest::Group {
                            anime_ids,
                            in_library: false,
                        })
                    } else {
                        self.franchise_groups
                            .get(g_idx)
                            .map(|group| ContinueRequest::Group {
                                anime_ids: group
                                    .iter()
                                    .filter_map(|&idx| {
                                        self.search_results.get(idx).map(|anime| anime.id)
                                    })
                                    .collect(),
                                in_library: false,
                            })
                    }
                })
                .or(Some(ContinueRequest::Latest)),
        };
    }

    fn handle_pending_delete_confirmation(&mut self, key_code: KeyCode) -> bool {
        let Some((anime_ids, anime_title)) = self.pending_delete_confirmation.clone() else {
            return false;
        };

        match key_code {
            KeyCode::Char('y') => {
                self.pending_delete_confirmation = None;
                match self.storage.delete_anime_progresses(&anime_ids) {
                    Ok(()) => {
                        self.history = self.storage.load_history().unwrap_or_default();
                        self.reload_library_after_mutation();
                        self.set_info_status(format!("Прогрес для \"{}\" видалено", anime_title));
                    }
                    Err(e) => self.set_error_status(format!("Не вдалося видалити прогрес: {}", e)),
                }
                true
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.pending_delete_confirmation = None;
                true
            }
            _ => true,
        }
    }

    fn delete_library_selection(&mut self) {
        match self.mode {
            AppMode::Library => {
                if let Some(anime) = self.library_selected_anime() {
                    if !anime.anime_ids.is_empty() {
                        self.pending_delete_confirmation =
                            Some((anime.anime_ids.clone(), anime.anime_title.clone()));
                    }
                }
            }
            _ => {}
        }
    }

    fn toggle_library_selection_watched(&mut self) {
        match self.mode {
            AppMode::LibrarySeason | AppMode::LibraryDubbing => {
                let Some(anime_id) = self.library_selected_anime_id() else {
                    return;
                };
                let Some(season_num) = self.selected_season_num() else {
                    return;
                };
                let Some(anime_title) = self
                    .library_selected_anime()
                    .map(|anime| anime.anime_title.clone())
                else {
                    return;
                };

                let Some(sources) = self.current_sources.as_ref() else {
                    return;
                };

                let mut target_episodes = Vec::new();
                for studio in sources.ashdi.iter().filter(|s| s.season_number == season_num) {
                    for episode in &studio.episodes {
                        target_episodes.push((studio.studio_name.clone(), episode.episode_number));
                    }
                }
                target_episodes.sort_unstable();
                target_episodes.dedup();

                if target_episodes.is_empty() {
                    return;
                }

                let has_any_real_progress = target_episodes.iter().any(|(studio_name, episode)| {
                    let key = crate::storage::StorageManager::make_progress_key(anime_id, season_num, *episode, studio_name);
                    self.history.progress.get(&key).is_some_and(|p| p.watched || p.timestamp >= 10.0)
                });
                let mark_watched = !has_any_real_progress;

                for (studio_name, episode_number) in &target_episodes {
                    let result = if mark_watched {
                        self.storage.set_episode_watched(
                            anime_id,
                            &anime_title,
                            season_num,
                            *episode_number,
                            studio_name,
                            true,
                        )
                    } else {
                        self.storage.set_episode_watched(
                            anime_id,
                            &anime_title,
                            season_num,
                            *episode_number,
                            studio_name,
                            false,
                        )
                    };
                    if let Err(e) = result {
                        self.set_error_status(format!("Не вдалося оновити сезон: {}", e));
                        return;
                    }
                }

                self.history = self.storage.load_history().unwrap_or_default();
                self.reload_library_after_mutation();
                self.mode = if matches!(self.mode, AppMode::LibraryDubbing) {
                    AppMode::LibraryDubbing
                } else {
                    AppMode::LibrarySeason
                };
                self.set_info_status(if mark_watched {
                    format!("Сезон {} позначено як переглянутий", season_num)
                } else {
                    format!("Прогрес сезону {} очищено", season_num)
                });
            }
            AppMode::LibraryEpisode => {
                let Some(anime_id) = self.library_selected_anime_id() else {
                    return;
                };
                let Some(season_num) = self.selected_season_num() else {
                    return;
                };
                let Some(anime_title) = self
                    .library_selected_anime()
                    .map(|anime| anime.anime_title.clone())
                else {
                    return;
                };
                let Some(selected_studio) = self.selected_studio() else {
                    return;
                };
                let studio_name = selected_studio.studio_name.clone();
                let Some(episode_number) = self.selected_episode_index
                    .and_then(|ep_idx| selected_studio.episodes.get(ep_idx))
                    .map(|episode| episode.episode_number)
                else {
                    return;
                };

                let key = crate::storage::StorageManager::make_progress_key(anime_id, season_num, episode_number, &studio_name);
                let current_progress = self.history.progress.get(&key).cloned();

                let result = match current_progress.as_ref() {
                    Some(progress) if progress.watched => self.storage.set_episode_watched(
                        anime_id,
                        &anime_title,
                        season_num,
                        episode_number,
                        &studio_name,
                        false,
                    ),
                    Some(progress) if progress.timestamp >= 10.0 && progress.timestamp < 1200.0 => {
                        self.storage
                            .reset_episode_progress(anime_id, season_num, episode_number, &studio_name)
                    }
                    _ => self.storage.set_episode_watched(
                        anime_id,
                        &anime_title,
                        season_num,
                        episode_number,
                        &studio_name,
                        true,
                    ),
                };

                match result {
                    Ok(()) => {
                        self.history = self.storage.load_history().unwrap_or_default();
                        self.reload_library_after_mutation();
                        self.mode = AppMode::LibraryEpisode;
                        let message = match current_progress.as_ref() {
                            Some(progress) if progress.watched => {
                                format!(
                                    "Серію S{}E{} позначено як непереглянуту",
                                    season_num, episode_number
                                )
                            }
                            Some(progress)
                                if progress.timestamp >= 10.0 && progress.timestamp < 1200.0 =>
                            {
                                format!("Таймер для S{}E{} очищено", season_num, episode_number)
                            }
                            _ => {
                                format!(
                                    "Серію S{}E{} позначено як переглянуту",
                                    season_num, episode_number
                                )
                            }
                        };
                        self.set_info_status(message);
                    }
                    Err(e) => self.set_error_status(format!("Не вдалося оновити серію: {}", e)),
                }
            }
            _ => {}
        }
    }

    fn search_selected_season_anime_id(&self) -> Option<u32> {
        let season_num = self.selected_season_num()?;
        self.current_sources.as_ref().and_then(|sources| {
            sources
                .ashdi
                .iter()
                .position(|studio| studio.season_number == season_num)
                .and_then(|idx| self.studio_anime_ids.get(idx))
                .copied()
        })
    }

    fn selected_episode_number(&self) -> Option<u32> {
        let episode_idx = self.selected_episode_index?;
        self.selected_studio()
            .and_then(|studio| studio.episodes.get(episode_idx))
            .map(|episode| episode.episode_number)
    }

    fn toggle_search_selection_watched(&mut self) {
        let anime_id = match self.focus {
            FocusPanel::SeasonList | FocusPanel::DubbingList | FocusPanel::EpisodeList => {
                self.search_selected_season_anime_id()
            }
            FocusPanel::SearchList => None,
        };
        let Some(anime_id) = anime_id else {
            return;
        };
        let Some(anime_title) = self
            .search_results
            .iter()
            .find(|anime| anime.id == anime_id)
            .map(|anime| anime.title_ukrainian.clone())
            .or_else(|| {
                self.current_details
                    .as_ref()
                    .map(|details| details.title_ukrainian.clone())
            })
        else {
            return;
        };
        let Some(season_num) = self.selected_season_num() else {
            return;
        };

        match self.focus {
            FocusPanel::SeasonList | FocusPanel::DubbingList => {
                let Some(sources) = self.current_sources.as_ref() else {
                    return;
                };
                let mut target_episodes = Vec::new();
                for studio in sources.ashdi.iter().filter(|s| s.season_number == season_num) {
                    for episode in &studio.episodes {
                        target_episodes.push((studio.studio_name.clone(), episode.episode_number));
                    }
                }
                target_episodes.sort_unstable();
                target_episodes.dedup();
                if target_episodes.is_empty() {
                    return;
                }

                let all_watched = target_episodes.iter().all(|(studio_name, episode)| {
                    self.history.progress.values().any(|progress| {
                        progress.anime_id == anime_id
                            && progress.season == season_num
                            && progress.episode == *episode
                            && progress.studio_name == *studio_name
                            && progress.watched
                    })
                });

                for (studio_name, episode_number) in &target_episodes {
                    let result = self.storage.set_episode_watched(
                        anime_id,
                        &anime_title,
                        season_num,
                        *episode_number,
                        studio_name,
                        !all_watched,
                    );
                    if let Err(e) = result {
                        self.set_error_status(format!("Не вдалося оновити сезон: {}", e));
                        return;
                    }
                }
                self.history = self.storage.load_history().unwrap_or_default();
                self.set_info_status(if all_watched {
                    format!("Сезон {} позначено як непереглянутий", season_num)
                } else {
                    format!("Сезон {} позначено як переглянутий", season_num)
                });
            }
            FocusPanel::EpisodeList => {
                let Some(selected_studio) = self.selected_studio() else {
                    return;
                };
                let studio_name = selected_studio.studio_name.clone();
                let Some(episode_number) = self.selected_episode_number() else {
                    return;
                };
                let key = crate::storage::StorageManager::make_progress_key(anime_id, season_num, episode_number, &studio_name);
                let current_progress = self.history.progress.get(&key).cloned();
                let result = match current_progress.as_ref() {
                    Some(progress) if progress.watched => self.storage.set_episode_watched(
                        anime_id,
                        &anime_title,
                        season_num,
                        episode_number,
                        &studio_name,
                        false,
                    ),
                    Some(progress) if progress.timestamp >= 10.0 && progress.timestamp < 1200.0 => {
                        self.storage
                            .reset_episode_progress(anime_id, season_num, episode_number, &studio_name)
                    }
                    _ => self.storage.set_episode_watched(
                        anime_id,
                        &anime_title,
                        season_num,
                        episode_number,
                        &studio_name,
                        true,
                    ),
                };
                match result {
                    Ok(()) => {
                        self.history = self.storage.load_history().unwrap_or_default();
                    }
                    Err(e) => self.set_error_status(format!("Не вдалося оновити серію: {}", e)),
                }
            }
            FocusPanel::SearchList => {}
        }
    }

    fn reload_library_after_mutation(&mut self) {
        let prev_anime_title = self
            .library_selected_anime()
            .map(|anime| anime.anime_title.clone());
        let prev_season = self.selected_season_num();
        let prev_dubbing = self.selected_dubbing_index;
        let prev_episode = self.selected_episode_index;
        let prev_mode = self.mode;

        self.library_items = build_library_items(&self.history);

        self.library_anime_index = prev_anime_title
            .clone()
            .and_then(|anime_title| {
                self.library_items
                    .iter()
                    .position(|item| item.anime_title == anime_title)
            })
            .or_else(|| (!self.library_items.is_empty()).then_some(0));
        self.library_anime_list_state
            .select(self.library_anime_index);

        if self.library_items.is_empty() {
            self.mode = AppMode::Library;
        } else if self.mode != AppMode::Library && self.library_selected_anime().is_none() {
            self.mode = AppMode::Library;
        }

        let should_reprepare = match (&prev_anime_title, self.library_selected_anime()) {
            (Some(prev_title), Some(anime)) => anime.anime_title != *prev_title,
            (Some(_), None) => true,
            _ => self.current_sources.is_none(),
        };

        if should_reprepare {
            self.prepare_library_anime_selection();
        }

        if let Some(season_num) = prev_season {
            self.selected_season_index = self
                .library_season_numbers()
                .iter()
                .position(|&s| s == season_num);
            self.season_list_state.select(self.selected_season_index);
        }
        if prev_mode == AppMode::LibraryDubbing || prev_mode == AppMode::LibraryEpisode {
            self.selected_dubbing_index = prev_dubbing.filter(|&idx| {
                self.selected_season_num()
                    .is_some_and(|sn| idx < self.studios_for_season(sn).len())
            });
            self.dubbing_list_state.select(self.selected_dubbing_index);
        }
        if prev_mode == AppMode::LibraryEpisode {
            self.selected_episode_index = prev_episode.filter(|&idx| {
                self.selected_studio()
                    .is_some_and(|studio| idx < studio.episodes.len())
            });
            self.episode_list_state.select(self.selected_episode_index);
        }

        self.mode = prev_mode;
        self.sync_library_sidebar_selection();
    }

    fn sync_library_sidebar_selection(&mut self) {
        let Some(anime_id) = self.library_selected_anime_id() else {
            self.current_poster = None;
            self.current_details = None;
            self.poster_fetch_pending = None;
            return;
        };

        self.current_details = self.details_cache.get(&anime_id).cloned();
        if let Some(img) = self.poster_cache.get(&anime_id).cloned() {
            self.current_poster = Some(self.picker.new_resize_protocol(img));
            self.poster_fetch_pending = None;
        } else {
            self.current_poster = None;
            self.poster_fetch_pending = Some(anime_id);
        }

        if self.current_details.as_ref().map(|details| details.id) != Some(anime_id) {
            self.loading = true;
        }
    }

    /// Оновлює sidebar_anime_idx і current_poster при зміні вибору у SeasonList.
    fn update_sidebar_for_season(&mut self) {
        let season_num = match self.selected_season_num() {
            Some(n) => n,
            None => return,
        };
        let j = self.current_sources.as_ref().and_then(|sources| {
            sources
                .ashdi
                .iter()
                .position(|s| s.season_number == season_num)
        });
        let j = match j {
            Some(j) => j,
            None => return,
        };
        let anime_id = match self.studio_anime_ids.get(j).copied() {
            Some(id) => id,
            None => return,
        };
        // Порівнюємо за anime_id, а не за позицією в search_results.
        // anime_id може не бути в search_results (напр. S3 "Вбивця романтики" при пошуку укр.),
        // тоді new_idx = None, але постер все одно треба оновити.
        let current_anime_id = self
            .sidebar_anime_idx
            .and_then(|i| self.search_results.get(i))
            .map(|a| a.id);
        if current_anime_id == Some(anime_id) {
            return;
        }
        self.sidebar_anime_idx = self.search_results.iter().position(|a| a.id == anime_id);
        self.current_details = self.details_cache.get(&anime_id).cloned();
        if self.current_details.is_none() {
            self.loading = true;
        }
        if let Some(img) = self.poster_cache.get(&anime_id).cloned() {
            self.current_poster = Some(self.picker.new_resize_protocol(img));
        } else {
            self.current_poster = None;
            self.poster_fetch_pending = Some(anime_id);
        }
    }

    /// Відновлює постер першого TV-члена франшизи при поверненні до SearchList.
    fn restore_representative_poster(&mut self) {
        // studio_anime_ids[0] — S1 після back-alignment (найточніший варіант)
        let first_tv_id = self.studio_anime_ids.first().copied().or_else(|| {
            self.selected_result_index
                .and_then(|i| self.search_results.get(i))
                .map(|item| item.id)
        });

        if let Some(id) = first_tv_id {
            if let Some(img) = self.poster_cache.get(&id).cloned() {
                self.current_poster = Some(self.picker.new_resize_protocol(img));
            } else {
                self.current_poster = None;
            }
        }
    }
}

fn build_library_items(history: &AppHistory) -> Vec<LibraryAnimeEntry> {
    let mut per_anime: HashMap<String, Vec<WatchProgress>> = HashMap::new();
    for progress in history.progress.values() {
        per_anime
            .entry(progress.anime_title.clone())
            .or_default()
            .push(progress.clone());
    }

    let mut items: Vec<LibraryAnimeEntry> = per_anime
        .into_iter()
        .filter_map(|(anime_title, progress_list)| {
            let latest_progress = progress_list
                .iter()
                .max_by_key(|progress| progress.updated_at)?
                .clone();

            let mut per_season: HashMap<u32, Vec<WatchProgress>> = HashMap::new();
            for progress in progress_list {
                per_season
                    .entry(progress.season)
                    .or_default()
                    .push(progress);
            }

            let mut seasons: Vec<LibrarySeasonEntry> = per_season
                .into_iter()
                .map(|(season, mut episodes)| {
                    episodes.sort_by_key(|progress| progress.episode);
                    let anime_id = episodes
                        .iter()
                        .max_by_key(|progress| progress.updated_at)
                        .map(|progress| progress.anime_id)
                        .unwrap_or(latest_progress.anime_id);
                    LibrarySeasonEntry {
                        anime_id,
                        season,
                        episodes,
                    }
                })
                .collect();
            seasons.sort_by_key(|entry| entry.season);
            let mut anime_ids: Vec<u32> = seasons.iter().map(|season| season.anime_id).collect();
            anime_ids.sort();
            anime_ids.dedup();

            Some(LibraryAnimeEntry {
                anime_ids,
                anime_title,
                latest_progress,
                seasons,
            })
        })
        .collect();

    items.sort_by(|a, b| {
        b.latest_progress
            .updated_at
            .cmp(&a.latest_progress.updated_at)
    });
    items
}

fn anime_is_fully_watched(anime: &LibraryAnimeEntry) -> bool {
    !anime.seasons.is_empty()
        && anime
            .seasons
            .iter()
            .all(|season| season.episodes.iter().all(|episode| episode.watched))
}
