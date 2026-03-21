use std::collections::HashMap;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::ListState;
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use image::DynamicImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use crate::api::{self, ApiClient, AnimeItem, AnimeDetails, EpisodeSourcesResponse, AshdiStudio};
use crate::api::anilist::FranchiseMember;
use crate::storage::StorageManager;

#[derive(PartialEq)]
pub enum AppMode {
    Normal,
    SearchInput,
}

#[derive(PartialEq)]
pub enum FocusPanel {
    SearchList,
    SeasonList,
    DubbingList,
    EpisodeList,
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

    pub should_quit: bool,
    pub api_client: ApiClient,
    pub storage: StorageManager,
    pub loading: bool,
    pub play_episode: bool,
    pub error_msg: Option<String>,

    // Стан відтворення
    pub is_playing: bool,
    pub mpv_child: Option<Child>,
    pub mpv_monitor: Option<JoinHandle<f64>>,
    pub pending_progress: Option<(u32, String, u32, u32)>,

    // Кеші та prefetch
    pub details_cache: HashMap<u32, AnimeDetails>,
    pub sources_cache: HashMap<u32, EpisodeSourcesResponse>,
    pub prefetching: bool,
    pub prefetch_rx: Option<mpsc::Receiver<(u32, Option<AnimeDetails>, Option<EpisodeSourcesResponse>)>>,

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

            should_quit: false,
            api_client: ApiClient::new()?,
            storage: StorageManager::new()?,
            loading: false,
            play_episode: false,
            error_msg: None,

            is_playing: false,
            mpv_child: None,
            mpv_monitor: None,
            pending_progress: None,

            details_cache: HashMap::new(),
            sources_cache: HashMap::new(),
            prefetching: false,
            prefetch_rx: None,

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
        let Some(sources) = &self.current_sources else { return Vec::new(); };
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
        let Some(sources) = &self.current_sources else { return Vec::new(); };
        sources.ashdi.iter().filter(|s| s.season_number == season_num).collect()
    }

    /// Поточний season_number за selected_season_index.
    pub fn selected_season_num(&self) -> Option<u32> {
        let idx = self.selected_season_index?;
        self.unique_seasons().get(idx).copied()
    }

    /// Обрана студія (для відтворення та списку епізодів).
    pub fn selected_studio(&self) -> Option<&AshdiStudio> {
        let season_num = self.selected_season_num()?;
        let dub_idx = self.selected_dubbing_index?;
        self.studios_for_season(season_num).into_iter().nth(dub_idx)
    }

    // ---

    pub fn handle_events(&mut self) -> anyhow::Result<()> {
        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match self.mode {
                        AppMode::Normal => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Esc => self.handle_esc(),
                            KeyCode::Char('/') => {
                                self.mode = AppMode::SearchInput;
                                self.search_query.clear();
                            },
                            KeyCode::Right => self.move_focus_right(),
                            KeyCode::Left  => self.move_focus_left(),
                            KeyCode::Down  => self.move_selection_down(),
                            KeyCode::Up    => self.move_selection_up(),
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
                            KeyCode::Char(c)  => self.search_query.push(c),
                            KeyCode::Backspace => { self.search_query.pop(); }
                            KeyCode::Esc => {
                                self.mode = AppMode::Normal;
                            },
                            _ => {}
                        },
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_esc(&mut self) {
        if self.error_msg.is_some() {
            self.error_msg = None;
            return;
        }
        match self.focus {
            FocusPanel::EpisodeList => self.focus = FocusPanel::DubbingList,
            FocusPanel::DubbingList => self.focus = FocusPanel::SeasonList,
            FocusPanel::SeasonList  => {
                self.focus = FocusPanel::SearchList;
                self.sidebar_anime_idx = None;
                self.restore_representative_poster();
            }
            FocusPanel::SearchList  => {}
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
                    let has_seasons = self.current_sources.as_ref()
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
            },
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
            },
            FocusPanel::DubbingList => {
                let has_episodes = self.selected_studio().map_or(false, |s| !s.episodes.is_empty());
                if has_episodes {
                    self.selected_episode_index = Some(0);
                    self.episode_list_state.select(Some(0));
                    FocusPanel::EpisodeList
                } else {
                    FocusPanel::DubbingList
                }
            },
            FocusPanel::EpisodeList => FocusPanel::EpisodeList,
        };
    }

    fn move_focus_left(&mut self) {
        self.handle_esc();
    }

    fn move_selection_down(&mut self) {
        match self.focus {
            FocusPanel::SearchList => {
                let total = self.franchise_groups.len();
                if total == 0 { return; }
                let i = match self.result_list_state.selected() {
                    Some(i) => if i >= total.saturating_sub(1) { 0 } else { i + 1 },
                    None => 0,
                };
                self.result_list_state.select(Some(i));
                self.selected_group_index = Some(i);
                if let Some(group) = self.franchise_groups.get(i) {
                    self.selected_result_index = Some(api::representative_idx(&self.search_results, group));
                }
                self.reset_downstream();
            }
            FocusPanel::SeasonList => {
                let total = self.unique_seasons().len();
                if total == 0 { return; }
                let i = match self.season_list_state.selected() {
                    Some(i) => if i >= total.saturating_sub(1) { 0 } else { i + 1 },
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
                    if studios_len == 0 { return; }
                    let i = match self.dubbing_list_state.selected() {
                        Some(i) => if i >= studios_len.saturating_sub(1) { 0 } else { i + 1 },
                        None => 0,
                    };
                    self.dubbing_list_state.select(Some(i));
                    self.selected_dubbing_index = Some(i);
                }
            }
            FocusPanel::EpisodeList => {
                let episodes_len = self.selected_studio().map_or(0, |s| s.episodes.len());
                if episodes_len == 0 { return; }
                let i = match self.episode_list_state.selected() {
                    Some(i) => if i >= episodes_len.saturating_sub(1) { 0 } else { i + 1 },
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
                if total == 0 { return; }
                let i = match self.result_list_state.selected() {
                    Some(i) => if i == 0 { total.saturating_sub(1) } else { i - 1 },
                    None => 0,
                };
                self.result_list_state.select(Some(i));
                self.selected_group_index = Some(i);
                if let Some(group) = self.franchise_groups.get(i) {
                    self.selected_result_index = Some(api::representative_idx(&self.search_results, group));
                }
                self.reset_downstream();
            }
            FocusPanel::SeasonList => {
                let total = self.unique_seasons().len();
                if total == 0 { return; }
                let i = match self.season_list_state.selected() {
                    Some(i) => if i == 0 { total.saturating_sub(1) } else { i - 1 },
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
                    if studios_len == 0 { return; }
                    let i = match self.dubbing_list_state.selected() {
                        Some(i) => if i == 0 { studios_len.saturating_sub(1) } else { i - 1 },
                        None => 0,
                    };
                    self.dubbing_list_state.select(Some(i));
                    self.selected_dubbing_index = Some(i);
                }
            }
            FocusPanel::EpisodeList => {
                let episodes_len = self.selected_studio().map_or(0, |s| s.episodes.len());
                if episodes_len == 0 { return; }
                let i = match self.episode_list_state.selected() {
                    Some(i) => if i == 0 { episodes_len.saturating_sub(1) } else { i - 1 },
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

    /// Оновлює sidebar_anime_idx і current_poster при зміні вибору у SeasonList.
    fn update_sidebar_for_season(&mut self) {
        let season_num = match self.selected_season_num() {
            Some(n) => n,
            None => return,
        };
        let j = self.current_sources.as_ref()
            .and_then(|sources| sources.ashdi.iter().position(|s| s.season_number == season_num));
        let j = match j { Some(j) => j, None => return };
        let anime_id = match self.studio_anime_ids.get(j).copied() {
            Some(id) => id,
            None => return,
        };
        // Порівнюємо за anime_id, а не за позицією в search_results.
        // anime_id може не бути в search_results (напр. S3 "Вбивця романтики" при пошуку укр.),
        // тоді new_idx = None, але постер все одно треба оновити.
        let current_anime_id = self.sidebar_anime_idx
            .and_then(|i| self.search_results.get(i))
            .map(|a| a.id);
        if current_anime_id == Some(anime_id) { return; }
        self.sidebar_anime_idx = self.search_results.iter().position(|a| a.id == anime_id);
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
        let first_tv_id = self.studio_anime_ids.first().copied()
            .or_else(|| {
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
