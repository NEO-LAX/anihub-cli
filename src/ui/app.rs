use crate::api::{
    AniListMedia, AnimeDetails, AnimeItem, ApiClient, AshdiStudio, EpisodeSourcesKey,
    EpisodeSourcesResponse, FranchiseCatalog, MoonAnimeBrowserEpisode, MoonAnimeSourceMarker,
    ReleaseAvailability, ReleaseEntry,
};
use crate::storage::{
    AnimeStatus, AppHistory, EpisodeWatchedUpdate, StorageManager, WatchProgress,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use image::DynamicImage;
use ratatui::widgets::ListState;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppMode {
    Normal,
    SearchInput,
    Library,
    LibrarySeason,
    LibraryDubbing,
    LibraryEpisode,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimaryTab {
    Search,
    Library,
    Settings,
}

impl PrimaryTab {
    pub const ALL: [Self; 3] = [Self::Search, Self::Library, Self::Settings];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Search => "Пошук",
            Self::Library => "Бібліотека",
            Self::Settings => "Налаштування",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum FocusPanel {
    SearchList,
    ReleaseList,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryFilter {
    All,
    Watching,
    Planned,
    Completed,
    OnHold,
    Dropped,
}

impl LibraryFilter {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Watching,
        Self::Planned,
        Self::Completed,
        Self::OnHold,
        Self::Dropped,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "Усі",
            Self::Watching => "Дивлюся",
            Self::Planned => "У планах",
            Self::Completed => "Переглянуто",
            Self::OnHold => "Відкладено",
            Self::Dropped => "Кинуто",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::All => Self::Watching,
            Self::Watching => Self::Planned,
            Self::Planned => Self::Completed,
            Self::Completed => Self::OnHold,
            Self::OnHold => Self::Dropped,
            Self::Dropped => Self::All,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::All => Self::Dropped,
            Self::Watching => Self::All,
            Self::Planned => Self::Watching,
            Self::Completed => Self::Planned,
            Self::OnHold => Self::Completed,
            Self::Dropped => Self::OnHold,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimeStatusEditor {
    pub anime_ids: Vec<u32>,
    pub title: String,
    pub selected: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub anime_title: String,
    pub season: u32,
    pub episode: u32,
    pub studio_name: String,
    pub position: f64,
    pub duration: f64,
}

#[derive(Clone)]
pub struct LibraryAnimeEntry {
    pub anime_ids: Vec<u32>,
    pub anime_title: String,
    pub latest_progress: WatchProgress,
    pub seasons: Vec<LibrarySeasonEntry>,
    pub status: AnimeStatus,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct LibrarySeasonEntry {
    pub anime_id: u32,
    pub season: u32,
    pub episodes: Vec<WatchProgress>,
}

pub type HistoryIndexes = (HashSet<(u32, u32, u32)>, HashMap<(u32, u32, u32), f64>);
const ANILIST_POSTER_SUBJECT_BIT: u32 = 0x8000_0000;

#[derive(Clone, Copy)]
pub enum DubbingChoice<'a> {
    Ashdi(&'a AshdiStudio),
    MoonAnime(&'a MoonAnimeSourceMarker),
}

#[derive(Clone, Copy)]
pub enum EpisodeChoice<'a> {
    Ashdi(&'a crate::api::AshdiEpisode),
    MoonAnime(&'a MoonAnimeBrowserEpisode),
}

impl EpisodeChoice<'_> {
    pub fn episode_number(&self) -> u32 {
        match self {
            Self::Ashdi(episode) => episode.episode_number,
            Self::MoonAnime(episode) => episode.episode_number,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::Ashdi(episode) => &episode.title,
            Self::MoonAnime(episode) => &episode.title,
        }
    }
}

impl DubbingChoice<'_> {
    pub fn studio_name(&self) -> &str {
        match self {
            Self::Ashdi(studio) => &studio.studio_name,
            Self::MoonAnime(studio) => &studio.studio_name,
        }
    }

    pub fn episodes_count(&self) -> u32 {
        match self {
            Self::Ashdi(studio) => studio.episodes_count,
            Self::MoonAnime(studio) => studio.episodes_count,
        }
    }

    pub fn is_moonanime(&self) -> bool {
        matches!(self, Self::MoonAnime(_))
    }
}

pub struct AppState {
    pub mode: AppMode,
    pub focus: FocusPanel,
    pub search_query: String,
    pub last_search_query: String,
    /// Cursor position in Unicode scalar values, not bytes.
    pub search_cursor: usize,

    pub search_results: Vec<AnimeItem>,
    pub franchise_groups: Vec<Vec<usize>>,
    /// Release catalogs aligned with `franchise_groups` by index.
    pub franchise_catalogs: Vec<FranchiseCatalog>,
    /// Relation metadata retained so catalogs can be rebuilt after an AniHub
    /// availability lookup completes.
    pub anilist_media: Vec<AniListMedia>,
    pub selected_group_index: Option<usize>,
    pub selected_result_index: Option<usize>,
    /// Selected release in the active search catalog. Library navigation keeps
    /// using `selected_season_index` for its persisted raw season numbers.
    pub selected_release_index: Option<usize>,
    pub selected_season_index: Option<usize>,
    pub selected_dubbing_index: Option<usize>,
    pub selected_episode_index: Option<usize>,

    pub current_sources: Option<EpisodeSourcesResponse>,
    /// Exact AniHub release and queried franchise season that own
    /// `current_sources`. Both must match before episodes may be rendered.
    pub current_sources_key: Option<EpisodeSourcesKey>,
    pub current_details: Option<AnimeDetails>,

    // Для кожного studio entry в current_sources.ashdi — який anime_id він представляє
    pub studio_anime_ids: Vec<u32>,
    // Який search_result показувати в сайдбарі (None = selected_result_index)
    pub sidebar_anime_idx: Option<usize>,
    /// Cache subject whose metadata and poster belong in the sidebar.
    ///
    /// Unlike `sidebar_anime_idx`, this also represents releases that are not
    /// present in the current search response and provides an ownership guard
    /// for asynchronously completed poster requests.
    pub sidebar_subject_id: Option<u32>,
    pub result_list_state: ListState,
    pub season_list_state: ListState,
    pub dubbing_list_state: ListState,
    pub episode_list_state: ListState,

    pub library_items: Vec<LibraryAnimeEntry>,
    pub library_all_items: Vec<LibraryAnimeEntry>,
    pub library_filter: LibraryFilter,
    pub library_anime_index: Option<usize>,
    pub library_season_index: Option<usize>,
    pub library_episode_index: Option<usize>,
    pub library_anime_list_state: ListState,
    pub library_season_list_state: ListState,
    pub library_episode_list_state: ListState,
    pub pending_delete_confirmation: Option<(Vec<u32>, String)>,
    pub status_editor: Option<AnimeStatusEditor>,

    pub should_quit: bool,
    pub api_client: ApiClient,
    pub storage: StorageManager,
    pub history: AppHistory,
    pub loading: bool,
    pub activity_message: Option<String>,
    pub play_episode: bool,
    pub continue_request: Option<ContinueRequest>,
    pub status_message: Option<(String, StatusKind)>,
    pub status_expires_at: Option<Instant>,
    pub show_help: bool,
    /// (display title, direct MoonAnime iframe URL)
    pub moonanime_browser_prompt: Option<(String, String)>,

    pub now_playing: Option<NowPlaying>,

    // Кеші ресурсів
    pub details_cache: moka::sync::Cache<u32, AnimeDetails>,
    pub sources_cache: moka::sync::Cache<EpisodeSourcesKey, EpisodeSourcesResponse>,

    // Обкладинка
    pub picker: Picker,
    pub current_poster: Option<StatefulProtocol>,
    pub poster_cache: moka::sync::Cache<u32, std::sync::Arc<DynamicImage>>,
    pub poster_fetch_pending: Option<u32>,

    // O(1) індекси для перевірки переглянутих серій під час рендеру.
    // Ребілдяться щоразу коли змінюється `history`.
    /// (anime_id, season, episode) → true якщо watched
    pub watched_index: HashSet<(u32, u32, u32)>,
    /// (anime_id, season, episode) → timestamp якщо в процесі перегляду (не watched, >= 10s)
    pub progress_index: HashMap<(u32, u32, u32), f64>,
}

impl AppState {
    pub fn new(picker: Picker) -> anyhow::Result<Self> {
        let storage = StorageManager::new()?;
        let history = storage.load_history()?;
        let (watched_index, progress_index) = Self::build_history_indexes(&history);

        Ok(Self {
            mode: AppMode::SearchInput,
            focus: FocusPanel::SearchList,
            search_query: String::new(),
            last_search_query: String::new(),
            search_cursor: 0,

            search_results: Vec::new(),
            franchise_groups: Vec::new(),
            franchise_catalogs: Vec::new(),
            anilist_media: Vec::new(),
            selected_group_index: None,
            selected_result_index: None,
            selected_release_index: None,
            selected_season_index: None,
            selected_dubbing_index: None,
            selected_episode_index: None,
            current_sources: None,
            current_sources_key: None,
            current_details: None,

            studio_anime_ids: Vec::new(),
            sidebar_anime_idx: None,
            sidebar_subject_id: None,
            result_list_state: ListState::default(),
            season_list_state: ListState::default(),
            dubbing_list_state: ListState::default(),
            episode_list_state: ListState::default(),

            library_items: Vec::new(),
            library_all_items: Vec::new(),
            library_filter: LibraryFilter::All,
            library_anime_index: None,
            library_season_index: None,
            library_episode_index: None,
            library_anime_list_state: ListState::default(),
            library_season_list_state: ListState::default(),
            library_episode_list_state: ListState::default(),
            pending_delete_confirmation: None,
            status_editor: None,

            should_quit: false,
            api_client: ApiClient::new()?,
            storage,
            history,
            loading: false,
            activity_message: None,
            play_episode: false,
            continue_request: None,
            status_message: None,
            status_expires_at: None,
            show_help: false,
            moonanime_browser_prompt: None,

            now_playing: None,

            details_cache: moka::sync::Cache::builder().max_capacity(100).build(),
            sources_cache: moka::sync::Cache::builder().max_capacity(100).build(),

            picker,
            current_poster: None,
            poster_cache: moka::sync::Cache::builder().max_capacity(30).build(),
            poster_fetch_pending: None,

            watched_index,
            progress_index,
        })
    }

    /// Будує O(1) індекси з AppHistory.
    /// watched_index: (anime_id, season, episode) — переглянуті серії.
    /// progress_index: (anime_id, season, episode) → timestamp — серії в процесі (>= 10s, не watched).
    pub fn build_history_indexes(history: &AppHistory) -> HistoryIndexes {
        let mut watched = HashSet::new();
        let mut progress = HashMap::new();
        for p in history.progress.values() {
            let key = (p.anime_id, p.season, p.episode);
            if p.watched {
                watched.insert(key);
            } else if p.timestamp >= 10.0 {
                // Беремо максимальний timestamp якщо кілька студій для одного епізоду
                progress
                    .entry(key)
                    .and_modify(|t: &mut f64| *t = t.max(p.timestamp))
                    .or_insert(p.timestamp);
            }
        }
        (watched, progress)
    }

    /// Перебудовує індекси після зміни history.
    pub fn rebuild_history_indexes(&mut self) {
        let (watched, progress) = Self::build_history_indexes(&self.history);
        self.watched_index = watched;
        self.progress_index = progress;
    }

    /// Cache subject currently represented by the sidebar.
    ///
    /// The explicit subject takes precedence over the legacy search-result
    /// index so releases missing from search results can still own metadata
    /// and poster state.
    pub fn sidebar_subject(&self) -> Option<u32> {
        self.sidebar_subject_id
            .or_else(|| {
                self.sidebar_anime_idx
                    .and_then(|index| self.search_results.get(index))
                    .map(|anime| anime.id)
            })
            .or_else(|| {
                self.selected_result_index
                    .and_then(|index| self.search_results.get(index))
                    .map(|anime| anime.id)
            })
    }

    /// Select the release whose metadata and poster should appear in the
    /// sidebar. This is the single state transition used by release browsing.
    pub fn select_sidebar_subject(&mut self, anime_id: Option<u32>) {
        self.sidebar_subject_id = anime_id;
        self.sidebar_anime_idx =
            anime_id.and_then(|id| self.search_results.iter().position(|anime| anime.id == id));
        self.current_details = anime_id.and_then(|id| self.details_cache.get(&id));

        match anime_id {
            Some(id) => {
                if let Some(image) = self.poster_cache.get(&id) {
                    self.current_poster = Some(self.picker.new_resize_protocol((*image).clone()));
                    self.poster_fetch_pending = None;
                } else {
                    self.current_poster = None;
                    self.poster_fetch_pending = Some(id);
                }
            }
            None => {
                self.current_poster = None;
                self.poster_fetch_pending = None;
            }
        }
    }

    /// Whether an asynchronously completed poster still owns the sidebar.
    pub fn accepts_poster(&self, anime_id: u32) -> bool {
        self.sidebar_subject() == Some(anime_id)
    }

    /// Cache a completed poster and only install it when its release is still
    /// selected. Callers can safely use this for stale async completions.
    pub fn install_poster(&mut self, anime_id: u32, image: std::sync::Arc<DynamicImage>) -> bool {
        self.poster_cache.insert(anime_id, image.clone());
        if !self.accepts_poster(anime_id) {
            return false;
        }
        self.current_poster = Some(self.picker.new_resize_protocol((*image).clone()));
        self.poster_fetch_pending = None;
        true
    }

    pub fn selected_franchise_catalog(&self) -> Option<&FranchiseCatalog> {
        if self.is_library_mode() {
            return None;
        }
        self.selected_group_index
            .and_then(|index| self.franchise_catalogs.get(index))
    }

    pub fn has_release_catalog(&self) -> bool {
        self.selected_franchise_catalog()
            .is_some_and(|catalog| !catalog.releases.is_empty())
    }

    pub fn selected_release(&self) -> Option<&ReleaseEntry> {
        let index = self.selected_release_index?;
        self.selected_franchise_catalog()?.releases.get(index)
    }

    pub fn release_count(&self) -> usize {
        self.selected_franchise_catalog().map_or_else(
            || self.unique_seasons().len(),
            |catalog| catalog.releases.len(),
        )
    }

    pub fn selected_release_available(&self) -> bool {
        self.selected_release().is_some_and(|release| {
            release.availability == ReleaseAvailability::Available && release.anihub_id.is_some()
        })
    }

    pub fn selected_release_anihub_id(&self) -> Option<u32> {
        self.selected_release()
            .filter(|release| release.availability == ReleaseAvailability::Available)
            .and_then(|release| release.anihub_id)
    }

    /// Stable poster/cache subject for a release. Unavailable AniList-only
    /// releases use a disjoint synthetic key and must never feed playback,
    /// details, bookmark, or browser actions.
    pub fn selected_release_sidebar_subject(&self) -> Option<u32> {
        sidebar_subject_for_release(self.selected_release()?)
    }

    pub fn selected_release_source_key(&self) -> Option<EpisodeSourcesKey> {
        let release = self.selected_release()?;
        release.anihub_id.map(|anime_id| {
            EpisodeSourcesKey::new(anime_id, release.conceptual_season.unwrap_or(1))
        })
    }

    pub fn source_key_for_anime_id(&self, anime_id: u32) -> EpisodeSourcesKey {
        let season = self
            .franchise_catalogs
            .iter()
            .flat_map(|catalog| catalog.releases.iter())
            .find(|release| release.anihub_id == Some(anime_id))
            .and_then(|release| release.conceptual_season)
            .unwrap_or(1);
        EpisodeSourcesKey::new(anime_id, season)
    }

    pub fn poster_url_for_subject(&self, subject: u32) -> Option<String> {
        self.selected_franchise_catalog()
            .into_iter()
            .flat_map(|catalog| catalog.releases.iter())
            .find(|release| sidebar_subject_for_release(release) == Some(subject))
            .and_then(|release| release.poster_url.clone())
    }

    pub fn canonical_sidebar_subject(&self) -> Option<u32> {
        let catalog = self.selected_franchise_catalog()?;
        catalog
            .anchor_anilist_id
            .and_then(|anchor| {
                catalog
                    .releases
                    .iter()
                    .find(|release| release.anilist_id == Some(anchor))
            })
            .or_else(|| catalog.releases.first())
            .and_then(sidebar_subject_for_release)
    }

    fn initial_release_index(&self) -> Option<usize> {
        let catalog = self.selected_franchise_catalog()?;
        catalog
            .releases
            .iter()
            .position(|release| release.availability == ReleaseAvailability::Available)
            .or_else(|| (!catalog.releases.is_empty()).then_some(0))
    }

    pub fn select_release(&mut self, index: Option<usize>) {
        self.selected_release_index = index;
        self.season_list_state.select(index);
        self.selected_dubbing_index = None;
        self.dubbing_list_state.select(None);
        self.selected_episode_index = None;
        self.episode_list_state.select(None);

        let source_key = self.selected_release_source_key();
        let anime_id = source_key.map(|key| key.anime_id);
        let sidebar_subject = self.selected_release_sidebar_subject();
        self.current_sources = source_key.and_then(|key| self.sources_cache.get(&key));
        self.current_sources_key = self.current_sources.as_ref().and(source_key);
        self.studio_anime_ids = self
            .current_sources
            .as_ref()
            .map_or_else(Vec::new, |sources| {
                vec![anime_id.expect("cached release sources have an owner"); sources.ashdi.len()]
            });
        self.select_sidebar_subject(sidebar_subject);
        match (anime_id, self.current_sources.is_some()) {
            (Some(_), false) => {
                self.loading = true;
                self.activity_message = Some("Завантаження вибраного випуску…".to_string());
            }
            (Some(_), true) | (None, _) => {
                self.loading = false;
                self.activity_message = None;
            }
        }
    }

    pub fn refresh_selected_release(&mut self) {
        self.select_release(self.selected_release_index);
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
        for studio in &sources.moonanime {
            if !seasons.contains(&studio.season_number) {
                seasons.push(studio.season_number);
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

    /// Dubbing choices in provider priority order. Ashdi always wins; a
    /// MoonAnime studio with the same normalized name is not duplicated.
    pub fn dubbing_choices_for_season(&self, season_num: u32) -> Vec<DubbingChoice<'_>> {
        let Some(sources) = &self.current_sources else {
            return Vec::new();
        };
        dubbing_choices_for_sources(sources, season_num)
    }

    /// Поточний season_number за selected_season_index.
    pub fn selected_season_num(&self) -> Option<u32> {
        if self.is_library_mode() {
            let idx = self.selected_season_index?;
            self.library_season_numbers().get(idx).copied()
        } else if self.has_release_catalog() {
            (self.selected_release_available()
                && self.current_sources_key == self.selected_release_source_key())
            .then(|| self.unique_seasons().into_iter().next())
            .flatten()
        } else {
            let idx = self.selected_season_index?;
            self.unique_seasons().get(idx).copied()
        }
    }

    /// Обрана студія (для відтворення та списку епізодів).
    pub fn selected_studio(&self) -> Option<&AshdiStudio> {
        match self.selected_dubbing_choice()? {
            DubbingChoice::Ashdi(studio) => Some(studio),
            DubbingChoice::MoonAnime(_) => None,
        }
    }

    pub fn selected_dubbing_choice(&self) -> Option<DubbingChoice<'_>> {
        let season_num = self.selected_season_num()?;
        let dub_idx = self.selected_dubbing_index?;
        self.dubbing_choices_for_season(season_num)
            .into_iter()
            .nth(dub_idx)
    }

    pub fn selected_episode_choices(&self) -> Vec<EpisodeChoice<'_>> {
        match self.selected_dubbing_choice() {
            Some(DubbingChoice::Ashdi(studio)) => {
                studio.episodes.iter().map(EpisodeChoice::Ashdi).collect()
            }
            Some(DubbingChoice::MoonAnime(studio)) => studio
                .episodes
                .iter()
                .map(EpisodeChoice::MoonAnime)
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn selected_episode_count(&self) -> usize {
        self.selected_episode_choices().len()
    }

    fn selected_dubbing_is_moonanime(&self) -> bool {
        let Some(season) = self.selected_season_num() else {
            return false;
        };
        let Some(index) = self.selected_dubbing_index else {
            return false;
        };
        self.dubbing_choices_for_season(season)
            .get(index)
            .is_some_and(DubbingChoice::is_moonanime)
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

    pub const fn primary_tab(&self) -> PrimaryTab {
        match self.mode {
            AppMode::Library
            | AppMode::LibrarySeason
            | AppMode::LibraryDubbing
            | AppMode::LibraryEpisode => PrimaryTab::Library,
            AppMode::Settings => PrimaryTab::Settings,
            AppMode::Normal | AppMode::SearchInput => PrimaryTab::Search,
        }
    }

    fn switch_primary_tab(&mut self, tab: PrimaryTab) {
        self.status_editor = None;
        self.pending_delete_confirmation = None;
        self.moonanime_browser_prompt = None;
        match tab {
            PrimaryTab::Search => self.reset_to_home(),
            PrimaryTab::Library => self.open_library(),
            PrimaryTab::Settings => {
                self.mode = AppMode::Settings;
                self.clear_activity();
                self.clear_status();
            }
        }
    }

    fn handle_primary_tab_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match code {
            KeyCode::Char('1')
                if self.mode != AppMode::SearchInput
                    || modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.switch_primary_tab(PrimaryTab::Search);
            }
            KeyCode::Char('2')
                if self.mode != AppMode::SearchInput
                    || modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.switch_primary_tab(PrimaryTab::Library);
            }
            KeyCode::Char('3')
                if self.mode != AppMode::SearchInput
                    || modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.switch_primary_tab(PrimaryTab::Settings);
            }
            _ => return false,
        }
        true
    }

    // ---

    pub fn handle_events(&mut self) -> anyhow::Result<()> {
        self.clear_expired_status();

        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        self.should_quit = true;
                        return Ok(());
                    }
                    if matches!(self.status_message, Some((_, StatusKind::Error))) {
                        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                            self.clear_status();
                        }
                        return Ok(());
                    }
                    if self.show_help {
                        self.show_help = false;
                        return Ok(());
                    }

                    if self.handle_moonanime_browser_prompt(key.code) {
                        return Ok(());
                    }

                    if self.handle_status_editor(key.code) {
                        return Ok(());
                    }

                    if self.handle_pending_delete_confirmation(key.code) {
                        return Ok(());
                    }

                    if self.handle_primary_tab_key(key.code, key.modifiers) {
                        return Ok(());
                    }
                    self.clear_info_status();

                    if self.mode != AppMode::SearchInput
                        && (key.code == KeyCode::Char('?') || key.code == KeyCode::Char('h'))
                    {
                        self.show_help = true;
                        return Ok(());
                    }

                    if !matches!(self.mode, AppMode::SearchInput | AppMode::Settings)
                        && self.handle_list_navigation_key(key.code)
                    {
                        return Ok(());
                    }

                    match self.mode {
                        AppMode::Normal => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char('c') => self.request_continue(),
                            KeyCode::Char(' ') => self.toggle_search_selection_watched(),
                            KeyCode::Backspace => self.clear_selected_episode_timestamp(),
                            KeyCode::Char('e') => self.open_status_editor(),
                            KeyCode::Char('o') => self.open_in_browser(),
                            KeyCode::Char('l') => self.open_library(),
                            KeyCode::Esc => self.handle_esc(),
                            KeyCode::Char('/') => {
                                self.mode = AppMode::SearchInput;
                                self.search_query.clone_from(&self.last_search_query);
                                self.search_cursor = self.search_query.chars().count();
                                self.clear_activity();
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
                                let query = self.search_query.trim().to_string();
                                if !query.is_empty() {
                                    self.last_search_query.clone_from(&query);
                                    self.search_query = query;
                                    self.search_cursor = self.search_query.chars().count();
                                    self.loading = true;
                                    self.activity_message = Some("Пошук аніме…".to_string());
                                }
                            }
                            KeyCode::Char(c) => self.insert_search_char(c),
                            KeyCode::Backspace => self.backspace_search_char(),
                            KeyCode::Delete => self.delete_search_char(),
                            KeyCode::Left => {
                                self.search_cursor = self.search_cursor.saturating_sub(1);
                            }
                            KeyCode::Right => {
                                self.search_cursor =
                                    (self.search_cursor + 1).min(self.search_query.chars().count());
                            }
                            KeyCode::Home => self.search_cursor = 0,
                            KeyCode::End => self.search_cursor = self.search_query.chars().count(),
                            KeyCode::Esc => {
                                self.mode = AppMode::Normal;
                                self.search_query.clear();
                                self.search_cursor = 0;
                                self.clear_activity();
                            }
                            _ => {}
                        },
                        AppMode::Library => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char('c') => self.request_continue(),
                            KeyCode::Char('d') => self.delete_library_selection(),
                            KeyCode::Char('e') => self.open_status_editor(),
                            KeyCode::Char('o') => self.open_in_browser(),
                            KeyCode::Char('/') => self.open_global_search(),
                            KeyCode::Tab => self.cycle_library_filter(false),
                            KeyCode::BackTab => self.cycle_library_filter(true),
                            KeyCode::Left => {}
                            KeyCode::Esc => self.reset_to_home(),
                            KeyCode::Up => self.move_library_up(),
                            KeyCode::Down => self.move_library_down(),
                            KeyCode::Right | KeyCode::Enter => self.enter_library_season(),
                            _ => {}
                        },
                        AppMode::LibrarySeason => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char(' ') => self.toggle_library_selection_watched(),
                            KeyCode::Char('e') => self.open_status_editor(),
                            KeyCode::Char('o') => self.open_in_browser(),
                            KeyCode::Char('/') => self.open_global_search(),
                            KeyCode::Tab => self.cycle_library_filter(false),
                            KeyCode::BackTab => self.cycle_library_filter(true),
                            KeyCode::Left => self.leave_library_level(),
                            KeyCode::Esc => self.leave_library_level(),
                            KeyCode::Up => self.move_library_up(),
                            KeyCode::Down => self.move_library_down(),
                            KeyCode::Right | KeyCode::Enter => self.enter_library_dubbing(),
                            _ => {}
                        },
                        AppMode::LibraryDubbing => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char(' ') => self.toggle_library_selection_watched(),
                            KeyCode::Char('e') => self.open_status_editor(),
                            KeyCode::Char('o') => self.open_in_browser(),
                            KeyCode::Char('/') => self.open_global_search(),
                            KeyCode::Tab => self.cycle_library_filter(false),
                            KeyCode::BackTab => self.cycle_library_filter(true),
                            KeyCode::Left => self.leave_library_level(),
                            KeyCode::Esc => self.leave_library_level(),
                            KeyCode::Up => self.move_library_up(),
                            KeyCode::Down => self.move_library_down(),
                            KeyCode::Right | KeyCode::Enter => self.enter_library_episode(),
                            _ => {}
                        },
                        AppMode::LibraryEpisode => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Char(' ') => self.toggle_library_selection_watched(),
                            KeyCode::Backspace => self.clear_selected_episode_timestamp(),
                            KeyCode::Char('e') => self.open_status_editor(),
                            KeyCode::Char('o') => self.open_in_browser(),
                            KeyCode::Char('/') => self.open_global_search(),
                            KeyCode::Tab => self.cycle_library_filter(false),
                            KeyCode::BackTab => self.cycle_library_filter(true),
                            KeyCode::Left => self.leave_library_level(),
                            KeyCode::Esc => self.leave_library_level(),
                            KeyCode::Up => self.move_library_up(),
                            KeyCode::Down => self.move_library_down(),
                            KeyCode::Enter => self.activate_selected_episode(),
                            _ => {}
                        },
                        AppMode::Settings => match key.code {
                            KeyCode::Char('q') => self.should_quit = true,
                            KeyCode::Esc => self.switch_primary_tab(PrimaryTab::Search),
                            _ => {}
                        },
                    }
                }
            }
        }
        Ok(())
    }

    fn insert_search_char(&mut self, character: char) {
        let byte_index = byte_index_for_char(&self.search_query, self.search_cursor);
        self.search_query.insert(byte_index, character);
        self.search_cursor += 1;
    }

    fn backspace_search_char(&mut self) {
        if self.search_cursor == 0 {
            return;
        }
        let start = byte_index_for_char(&self.search_query, self.search_cursor - 1);
        let end = byte_index_for_char(&self.search_query, self.search_cursor);
        self.search_query.replace_range(start..end, "");
        self.search_cursor -= 1;
    }

    fn delete_search_char(&mut self) {
        let char_count = self.search_query.chars().count();
        if self.search_cursor >= char_count {
            return;
        }
        let start = byte_index_for_char(&self.search_query, self.search_cursor);
        let end = byte_index_for_char(&self.search_query, self.search_cursor + 1);
        self.search_query.replace_range(start..end, "");
    }

    fn handle_list_navigation_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.move_active_selection(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_active_selection(false),
            KeyCode::PageDown => self.move_active_page(true),
            KeyCode::PageUp => self.move_active_page(false),
            KeyCode::Home => self.jump_active_selection(false),
            KeyCode::End => self.jump_active_selection(true),
            _ => return false,
        }
        true
    }

    fn move_active_selection(&mut self, down: bool) {
        if self.is_library_mode() {
            if down {
                self.move_library_down();
            } else {
                self.move_library_up();
            }
        } else if down {
            self.move_selection_down();
        } else {
            self.move_selection_up();
        }
    }

    fn move_active_page(&mut self, down: bool) {
        for _ in 0..10 {
            let (selected, total) = self.active_list_position();
            if total == 0 || (down && selected + 1 >= total) || (!down && selected == 0) {
                break;
            }
            self.move_active_selection(down);
        }
    }

    fn jump_active_selection(&mut self, to_end: bool) {
        loop {
            let (selected, total) = self.active_list_position();
            if total == 0 || (!to_end && selected == 0) || (to_end && selected + 1 >= total) {
                break;
            }
            self.move_active_selection(to_end);
        }
    }

    pub fn active_list_position(&self) -> (usize, usize) {
        if self.mode == AppMode::Settings {
            return (0, 0);
        }
        if self.is_library_mode() {
            return match self.mode {
                AppMode::Library => (
                    self.library_anime_list_state.selected().unwrap_or(0),
                    self.library_items.len(),
                ),
                AppMode::LibrarySeason => (
                    self.season_list_state.selected().unwrap_or(0),
                    self.library_season_numbers().len(),
                ),
                AppMode::LibraryDubbing => {
                    let total = self
                        .selected_season_num()
                        .map_or(0, |season| self.dubbing_choices_for_season(season).len());
                    (self.dubbing_list_state.selected().unwrap_or(0), total)
                }
                AppMode::LibraryEpisode => (
                    self.episode_list_state.selected().unwrap_or(0),
                    self.selected_episode_count(),
                ),
                _ => (0, 0),
            };
        }

        match self.focus {
            FocusPanel::SearchList => (
                self.result_list_state.selected().unwrap_or(0),
                self.franchise_groups.len(),
            ),
            FocusPanel::ReleaseList => (
                self.season_list_state.selected().unwrap_or(0),
                self.release_count(),
            ),
            FocusPanel::DubbingList => {
                let total = self
                    .selected_season_num()
                    .map_or(0, |season| self.dubbing_choices_for_season(season).len());
                (self.dubbing_list_state.selected().unwrap_or(0), total)
            }
            FocusPanel::EpisodeList => (
                self.episode_list_state.selected().unwrap_or(0),
                self.selected_episode_count(),
            ),
        }
    }

    fn cycle_library_filter(&mut self, backwards: bool) {
        let filter = if backwards {
            self.library_filter.previous()
        } else {
            self.library_filter.next()
        };
        self.set_library_filter(filter);
    }

    fn set_library_filter(&mut self, filter: LibraryFilter) {
        self.library_filter = filter;
        self.mode = AppMode::Library;
        self.apply_library_filter();
    }

    fn apply_library_filter(&mut self) {
        let selected_ids = self
            .library_selected_anime()
            .map(|anime| anime.anime_ids.clone());
        self.library_items = self
            .library_all_items
            .iter()
            .filter(|anime| match self.library_filter {
                LibraryFilter::All => true,
                LibraryFilter::Watching => anime.status == AnimeStatus::Watching,
                LibraryFilter::Planned => anime.status == AnimeStatus::Planned,
                LibraryFilter::Completed => anime.status == AnimeStatus::Completed,
                LibraryFilter::OnHold => anime.status == AnimeStatus::OnHold,
                LibraryFilter::Dropped => anime.status == AnimeStatus::Dropped,
            })
            .cloned()
            .collect();
        self.library_anime_index = selected_ids
            .and_then(|ids| {
                self.library_items
                    .iter()
                    .position(|anime| anime.anime_ids.iter().any(|id| ids.contains(id)))
            })
            .or_else(|| (!self.library_items.is_empty()).then_some(0));
        self.library_anime_list_state
            .select(self.library_anime_index);
        self.library_season_index = None;
        self.library_episode_index = None;
        self.season_list_state.select(None);
        self.dubbing_list_state.select(None);
        self.episode_list_state.select(None);
        self.pending_delete_confirmation = None;
        if self.library_items.is_empty() {
            self.current_sources = None;
            self.current_sources_key = None;
            self.current_details = None;
            self.current_poster = None;
            self.loading = false;
            self.activity_message = None;
        } else {
            self.prepare_library_anime_selection();
        }
    }

    fn get_current_anime_context(&self) -> Option<(u32, String, String)> {
        if self.is_library_mode() {
            let anime = self.library_selected_anime()?;
            let anime_id = match self.mode {
                AppMode::LibrarySeason | AppMode::LibraryDubbing | AppMode::LibraryEpisode => {
                    self.selected_season_num()
                        .and_then(|sn| {
                            self.studios_for_season(sn).iter().find_map(|studio| {
                                if anime.anime_ids.contains(&studio.id) {
                                    // Not quite right, but we need the seasonal ID
                                    Some(studio.id)
                                } else {
                                    None
                                }
                            })
                        })
                        .unwrap_or(anime.latest_progress.anime_id)
                }
                _ => anime.latest_progress.anime_id,
            };

            let slug = self
                .details_cache
                .get(&anime_id)
                .map(|d| d.slug.clone())
                .unwrap_or_default();
            Some((anime_id, anime.anime_title.clone(), slug))
        } else {
            if self.focus != FocusPanel::SearchList && self.has_release_catalog() {
                let release = self.selected_release()?;
                let anime_id = self.selected_release_anihub_id()?;
                let slug = self
                    .details_cache
                    .get(&anime_id)
                    .map(|details| details.slug.clone())
                    .unwrap_or_default();
                return Some((anime_id, release.title.clone(), slug));
            }

            // Priority 1: Specifically selected seasonal ID if focused on details
            let anime_id = match self.focus {
                FocusPanel::ReleaseList | FocusPanel::DubbingList | FocusPanel::EpisodeList => {
                    self.search_selected_season_anime_id()
                }
                _ => None,
            };

            if let Some(id) = anime_id {
                if let Some(item) = self.search_results.iter().find(|i| i.id == id) {
                    return Some((id, item.title_ukrainian.clone(), item.slug.clone()));
                }
            }

            // Priority 2: Sidebar/Selected result index
            let idx = self.sidebar_anime_idx.or(self.selected_result_index)?;
            let item = self.search_results.get(idx)?;
            Some((item.id, item.title_ukrainian.clone(), item.slug.clone()))
        }
    }

    fn selected_anime_status_context(&self) -> Option<(Vec<u32>, String, AnimeStatus)> {
        let (mut anime_ids, title) = if self.is_library_mode() {
            let anime = self.library_selected_anime()?;
            return Some((
                anime.anime_ids.clone(),
                anime.anime_title.clone(),
                anime.status,
            ));
        } else {
            let group_index = self.selected_group_index?;
            if let Some(catalog) = self.franchise_catalogs.get(group_index) {
                (
                    catalog
                        .releases
                        .iter()
                        .filter_map(|release| release.anihub_id)
                        .collect::<Vec<_>>(),
                    catalog.canonical_title.clone(),
                )
            } else {
                let group = self.franchise_groups.get(group_index)?;
                let representative = group
                    .first()
                    .and_then(|index| self.search_results.get(*index))?;
                (
                    group
                        .iter()
                        .filter_map(|index| self.search_results.get(*index).map(|anime| anime.id))
                        .collect::<Vec<_>>(),
                    representative.title_ukrainian.clone(),
                )
            }
        };
        anime_ids.sort_unstable();
        anime_ids.dedup();
        if anime_ids.is_empty() {
            return None;
        }

        let explicit = anime_ids
            .iter()
            .filter_map(|anime_id| self.history.library.get(anime_id))
            .max_by_key(|record| record.updated_at)
            .map(|record| record.status);
        let status = explicit.unwrap_or_else(|| {
            let progress = self
                .history
                .progress
                .values()
                .filter(|progress| anime_ids.contains(&progress.anime_id))
                .collect::<Vec<_>>();
            if progress.is_empty() {
                AnimeStatus::NotAdded
            } else if progress.iter().all(|progress| progress.watched) {
                AnimeStatus::Completed
            } else {
                AnimeStatus::Watching
            }
        });
        Some((anime_ids, title, status))
    }

    fn open_status_editor(&mut self) {
        let Some((anime_ids, title, status)) = self.selected_anime_status_context() else {
            return;
        };
        let selected = AnimeStatus::ALL
            .iter()
            .position(|candidate| *candidate == status)
            .unwrap_or(0);
        self.status_editor = Some(AnimeStatusEditor {
            anime_ids,
            title,
            selected,
        });
    }

    fn handle_status_editor(&mut self, key_code: KeyCode) -> bool {
        let Some(editor) = self.status_editor.as_mut() else {
            return false;
        };
        match key_code {
            KeyCode::Up => {
                editor.selected = editor
                    .selected
                    .checked_sub(1)
                    .unwrap_or(AnimeStatus::ALL.len() - 1);
            }
            KeyCode::Down => editor.selected = (editor.selected + 1) % AnimeStatus::ALL.len(),
            KeyCode::Char(character @ '1'..='6') => {
                editor.selected = usize::from(character as u8 - b'1');
            }
            KeyCode::Enter => {
                let editor = self.status_editor.take().expect("status editor exists");
                let status = AnimeStatus::ALL[editor.selected];
                match self
                    .storage
                    .set_anime_status(&editor.anime_ids, &editor.title, status)
                {
                    Ok(history) => {
                        self.history = history;
                        self.rebuild_history_indexes();
                        if self.is_library_mode() {
                            self.reload_library_after_mutation();
                        }
                        self.set_info_status(format!("{}: {}", editor.title, status.label()));
                    }
                    Err(error) => {
                        self.set_error_status(format!("Не вдалося змінити статус: {error}"));
                    }
                }
            }
            KeyCode::Esc => self.status_editor = None,
            _ => {}
        }
        true
    }

    fn clear_selected_episode_timestamp(&mut self) {
        if !matches!(self.focus, FocusPanel::EpisodeList) && self.mode != AppMode::LibraryEpisode {
            return;
        }
        let Some(studio) = self.selected_studio() else {
            self.set_info_status("Для MoonAnime локального таймкоду немає");
            return;
        };
        let studio_name = studio.studio_name.clone();
        let Some(episode) = self.selected_episode_number() else {
            return;
        };
        let Some(season) = self.selected_season_num() else {
            return;
        };
        let anime_id = if self.is_library_mode() {
            self.library_selected_anime_id()
        } else {
            self.search_selected_season_anime_id()
        };
        let Some(anime_id) = anime_id else {
            return;
        };

        match self
            .storage
            .clear_episode_timestamp(anime_id, season, episode, &studio_name)
        {
            Ok(history) => {
                self.history = history;
                self.rebuild_history_indexes();
                self.set_info_status(format!("Таймкод S{season}E{episode} очищено"));
            }
            Err(error) => self.set_error_status(format!("Не вдалося очистити таймкод: {error}")),
        }
    }

    pub fn open_in_browser(&mut self) {
        if let Some((id, title, _)) = self.get_current_anime_context() {
            self.open_anime_in_browser(id, &title);
        }
    }

    fn open_anime_in_browser(&mut self, anime_id: u32, title: &str) {
        let url = format!("https://anihub.in.ua/anime/{anime_id}");
        self.open_url_in_browser(&url, title);
    }

    fn open_url_in_browser(&mut self, url: &str, title: &str) {
        let command =
            crate::platform::browser_open_command(crate::platform::Platform::current(), url);
        if std::process::Command::new(command.program)
            .args(command.args)
            .spawn()
            .is_ok()
        {
            self.set_info_status(format!("Відкрито в браузері: {title}"));
        } else {
            self.set_error_status("Не вдалося відкрити браузер");
        }
    }

    pub fn prompt_moonanime_browser(
        &mut self,
        title: impl Into<String>,
        iframe_url: impl Into<String>,
    ) {
        self.moonanime_browser_prompt = Some((title.into(), iframe_url.into()));
    }

    fn handle_moonanime_browser_prompt(&mut self, key_code: KeyCode) -> bool {
        let Some((title, iframe_url)) = self.moonanime_browser_prompt.clone() else {
            return false;
        };
        match key_code {
            KeyCode::Enter => {
                self.moonanime_browser_prompt = None;
                self.open_url_in_browser(&iframe_url, &title);
            }
            KeyCode::Esc => self.moonanime_browser_prompt = None,
            _ => {}
        }
        true
    }

    fn prompt_selected_moonanime_episode(&mut self) -> bool {
        if !self.selected_dubbing_is_moonanime() {
            return false;
        }
        let Some(index) = self.selected_episode_index else {
            return false;
        };
        let Some(EpisodeChoice::MoonAnime(episode)) =
            self.selected_episode_choices().get(index).copied()
        else {
            return false;
        };
        let studio_name = self
            .selected_dubbing_choice()
            .map(|choice| choice.studio_name().to_string())
            .unwrap_or_else(|| "MoonAnime".to_string());
        let title = self
            .current_details
            .as_ref()
            .map(|details| details.title_ukrainian.clone())
            .or_else(|| self.selected_release().map(|release| release.title.clone()))
            .or_else(|| {
                self.library_selected_anime()
                    .map(|anime| anime.anime_title.clone())
            })
            .unwrap_or_else(|| "Аніме".to_string());
        self.prompt_moonanime_browser(
            format!(
                "{title} — {studio_name} — серія {} [MoonAnime]",
                episode.episode_number
            ),
            episode.iframe_url.clone(),
        );
        true
    }

    fn activate_selected_episode(&mut self) {
        if !self.prompt_selected_moonanime_episode() {
            self.play_episode = true;
        }
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
            FocusPanel::SearchList => {} // Esc на SearchList — нічого не робимо
        }
    }

    fn handle_enter(&mut self) {
        if self.focus == FocusPanel::EpisodeList {
            self.activate_selected_episode();
        } else {
            self.move_focus_right();
        }
    }

    fn move_focus_right(&mut self) {
        self.focus = match self.focus {
            FocusPanel::SearchList => {
                if self.selected_result_index.is_some() {
                    if self.has_release_catalog() {
                        let index = self.initial_release_index();
                        self.select_release(index);
                        FocusPanel::ReleaseList
                    } else {
                        let has_seasons = self
                            .current_sources
                            .as_ref()
                            .is_some_and(|s| !s.ashdi.is_empty());
                        let seasons = self.unique_seasons();
                        if has_seasons && !seasons.is_empty() {
                            self.selected_season_index = Some(0);
                            self.season_list_state.select(Some(0));
                            self.update_sidebar_for_season();
                            if seasons.len() == 1 {
                                let season_num = seasons[0];
                                let studios_len = self.dubbing_choices_for_season(season_num).len();
                                if studios_len > 0 {
                                    self.selected_dubbing_index = Some(0);
                                    self.dubbing_list_state.select(Some(0));
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
                } else if let Some(sn) = self.selected_season_num() {
                    let studios_len = self.dubbing_choices_for_season(sn).len();
                    if studios_len > 0 {
                        self.selected_dubbing_index = Some(0);
                        self.dubbing_list_state.select(Some(0));
                        FocusPanel::DubbingList
                    } else {
                        FocusPanel::ReleaseList
                    }
                } else {
                    FocusPanel::ReleaseList
                }
            }
            FocusPanel::DubbingList => {
                let has_episodes = self.selected_episode_count() > 0;
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
        let total = self.release_count();
        if total == 0 {
            return;
        }
        let current = self.season_list_state.selected().unwrap_or(0);
        let next = if down {
            if current >= total.saturating_sub(1) {
                0
            } else {
                current + 1
            }
        } else if current == 0 {
            total.saturating_sub(1)
        } else {
            current - 1
        };

        if self.has_release_catalog() {
            self.select_release(Some(next));
        } else {
            self.season_list_state.select(Some(next));
            self.selected_season_index = Some(next);
            self.selected_dubbing_index = None;
            self.dubbing_list_state.select(None);
            self.update_sidebar_for_season();
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
                    self.selected_result_index = group.first().copied();
                }
                self.reset_downstream();
            }
            FocusPanel::ReleaseList => self.move_release_selection(true),
            FocusPanel::DubbingList => {
                if let Some(sn) = self.selected_season_num() {
                    let studios_len = self.dubbing_choices_for_season(sn).len();
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
                let episodes_len = self.selected_episode_count();
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
                    self.selected_result_index = group.first().copied();
                }
                self.reset_downstream();
            }
            FocusPanel::ReleaseList => self.move_release_selection(false),
            FocusPanel::DubbingList => {
                if let Some(sn) = self.selected_season_num() {
                    let studios_len = self.dubbing_choices_for_season(sn).len();
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
                let episodes_len = self.selected_episode_count();
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
        self.activity_message = Some("Завантаження вибраного аніме…".to_string());
        self.current_sources = None;
        self.current_sources_key = None;
        self.current_details = None;
        self.current_poster = None;
        self.studio_anime_ids.clear();
        self.sidebar_anime_idx = None;
        self.sidebar_subject_id = None;
        self.selected_release_index = None;
        self.selected_season_index = None;
        self.season_list_state.select(None);
        self.selected_dubbing_index = None;
        self.dubbing_list_state.select(None);
        self.selected_episode_index = None;
        self.episode_list_state.select(None);

        // Moving the search cursor changes the poster owner immediately.
        // Leaving the subject unset would keep the previous pending request,
        // whose completion is correctly rejected as stale, but no request for
        // the newly highlighted card would ever be scheduled until Enter.
        let subject = self.canonical_sidebar_subject().or_else(|| {
            self.selected_result_index
                .and_then(|index| self.search_results.get(index))
                .map(|item| item.id)
        });
        self.select_sidebar_subject(subject);
    }

    fn reset_to_home(&mut self) {
        self.mode = if self.search_results.is_empty() {
            AppMode::SearchInput
        } else {
            AppMode::Normal
        };
        self.focus = FocusPanel::SearchList;
        self.search_query.clear();
        self.search_cursor = 0;
        self.current_sources = None;
        self.current_sources_key = None;
        self.current_details = None;
        self.studio_anime_ids.clear();
        self.sidebar_anime_idx = None;
        self.sidebar_subject_id = None;
        self.result_list_state.select(self.selected_group_index);
        self.selected_release_index = None;
        self.selected_season_index = None;
        self.season_list_state.select(None);
        self.selected_dubbing_index = None;
        self.dubbing_list_state.select(None);
        self.selected_episode_index = None;
        self.episode_list_state.select(None);
        self.loading = self.selected_result_index.is_some();
        self.activity_message = self
            .loading
            .then(|| "Завантаження вибраного аніме…".to_string());
        self.clear_status();
        self.current_poster = None;
        self.poster_fetch_pending = None;
        self.library_items.clear();
        self.library_all_items.clear();
        self.library_anime_index = None;
        self.library_season_index = None;
        self.library_episode_index = None;
        self.library_anime_list_state.select(None);
        self.library_season_list_state.select(None);
        if self.mode == AppMode::Normal && self.selected_result_index.is_some() {
            self.restore_representative_poster();
        }
        self.library_episode_list_state.select(None);
        self.pending_delete_confirmation = None;
        self.status_editor = None;
    }

    fn open_global_search(&mut self) {
        self.mode = AppMode::SearchInput;
        self.focus = FocusPanel::SearchList;
        self.search_query.clone_from(&self.last_search_query);
        self.search_cursor = self.search_query.chars().count();
        self.pending_delete_confirmation = None;
        self.status_editor = None;
        self.clear_activity();
        self.clear_status();
    }

    pub fn open_library(&mut self) {
        if self.is_library_mode() {
            return;
        }

        self.library_all_items = build_library_items(&self.history);
        self.library_items.clear();
        self.mode = AppMode::Library;
        self.apply_library_filter();
        if self.library_all_items.is_empty() {
            self.set_info_status("Бібліотека порожня — додайте аніме через e");
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
        self.current_sources_key = None;
        self.current_details = None;
        self.current_poster = None;
        self.studio_anime_ids.clear();
        self.sync_library_sidebar_selection();
        self.loading = true;
        self.activity_message = Some("Завантаження бібліотеки…".to_string());
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
        if self.library_selected_anime().is_none()
            || (self.unique_seasons().is_empty()
                && self
                    .library_selected_anime()
                    .is_none_or(|anime| anime.seasons.is_empty()))
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
        if self.dubbing_choices_for_season(season_num).is_empty() {
            return;
        }

        self.mode = AppMode::LibraryDubbing;
        self.selected_dubbing_index = Some(0);
        self.selected_episode_index = None;
        self.dubbing_list_state.select(Some(0));
        self.episode_list_state.select(None);
    }

    fn enter_library_episode(&mut self) {
        if self.selected_episode_count() == 0 {
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
                let total = self.dubbing_choices_for_season(season_num).len();
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
                let total = self.selected_episode_count();
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
                let total = self.dubbing_choices_for_season(season_num).len();
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
                let total = self.selected_episode_count();
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
        self.activity_message = None;
        self.status_message = Some((message.into(), StatusKind::Error));
        self.status_expires_at = None;
    }

    pub fn set_activity(&mut self, message: impl Into<String>) {
        self.loading = true;
        self.activity_message = Some(message.into());
    }

    pub fn clear_activity(&mut self) {
        self.loading = false;
        self.activity_message = None;
    }

    pub fn prepare_playback(&mut self, target: &crate::playback::PlayTarget) {
        self.set_activity(format!(
            "Підготовка потоку · S{}E{}…",
            target.season, target.episode
        ));
        self.now_playing = Some(NowPlaying {
            anime_title: target.anime_title.clone(),
            season: target.season,
            episode: target.episode,
            studio_name: target.studio_name.clone(),
            position: target.start_time.unwrap_or(0.0),
            duration: 0.0,
        });
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
                        self.rebuild_history_indexes();
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
        if self.mode == AppMode::Library {
            if let Some(anime) = self.library_selected_anime() {
                if !anime.anime_ids.is_empty() {
                    self.pending_delete_confirmation =
                        Some((anime.anime_ids.clone(), anime.anime_title.clone()));
                }
            }
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
                for studio in sources
                    .ashdi
                    .iter()
                    .filter(|s| s.season_number == season_num)
                {
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
                    let key = crate::storage::StorageManager::make_progress_key(
                        anime_id,
                        season_num,
                        *episode,
                        studio_name,
                    );
                    self.history
                        .progress
                        .get(&key)
                        .is_some_and(|progress| progress.watched)
                });
                let mark_watched = !all_watched;

                let updates = target_episodes
                    .iter()
                    .map(|(studio_name, episode_number)| EpisodeWatchedUpdate {
                        anime_id,
                        anime_title: anime_title.clone(),
                        season: season_num,
                        episode: *episode_number,
                        studio_name: studio_name.clone(),
                        watched: mark_watched,
                    })
                    .collect::<Vec<_>>();
                match self.storage.set_episodes_watched(&updates) {
                    Ok(history) => self.history = history,
                    Err(e) => {
                        self.set_error_status(format!("Не вдалося оновити сезон: {}", e));
                        return;
                    }
                }
                self.rebuild_history_indexes();
                self.set_info_status(if mark_watched {
                    format!("Сезон {} позначено як переглянутий", season_num)
                } else {
                    format!("Сезон {} позначено як непереглянутий", season_num)
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
                let Some(episode_number) = self
                    .selected_episode_index
                    .and_then(|ep_idx| selected_studio.episodes.get(ep_idx))
                    .map(|episode| episode.episode_number)
                else {
                    return;
                };

                let key = crate::storage::StorageManager::make_progress_key(
                    anime_id,
                    season_num,
                    episode_number,
                    &studio_name,
                );
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
                    Ok(history) => {
                        self.history = history;
                        self.rebuild_history_indexes();
                        let message = match current_progress.as_ref() {
                            Some(progress) if progress.watched => {
                                format!(
                                    "Серію S{}E{} позначено як непереглянуту",
                                    season_num, episode_number
                                )
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
        if self.has_release_catalog() {
            return self.selected_release_anihub_id();
        }
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
            FocusPanel::ReleaseList | FocusPanel::DubbingList | FocusPanel::EpisodeList => {
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
            FocusPanel::ReleaseList | FocusPanel::DubbingList => {
                let Some(sources) = self.current_sources.as_ref() else {
                    return;
                };
                let mut target_episodes = Vec::new();
                for studio in sources
                    .ashdi
                    .iter()
                    .filter(|s| s.season_number == season_num)
                {
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

                let updates = target_episodes
                    .iter()
                    .map(|(studio_name, episode_number)| EpisodeWatchedUpdate {
                        anime_id,
                        anime_title: anime_title.clone(),
                        season: season_num,
                        episode: *episode_number,
                        studio_name: studio_name.clone(),
                        watched: !all_watched,
                    })
                    .collect::<Vec<_>>();
                match self.storage.set_episodes_watched(&updates) {
                    Ok(history) => self.history = history,
                    Err(e) => {
                        self.set_error_status(format!("Не вдалося оновити сезон: {}", e));
                        return;
                    }
                }
                self.rebuild_history_indexes();
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
                let key = crate::storage::StorageManager::make_progress_key(
                    anime_id,
                    season_num,
                    episode_number,
                    &studio_name,
                );
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
                    Ok(history) => {
                        self.history = history;
                        self.rebuild_history_indexes();
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

        self.library_all_items = build_library_items(&self.history);
        self.apply_library_filter();

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

        if self.library_items.is_empty()
            || (self.mode != AppMode::Library && self.library_selected_anime().is_none())
        {
            self.mode = AppMode::Library;
        }

        let should_reprepare = match (&prev_anime_title, self.library_selected_anime()) {
            (Some(prev_title), Some(anime)) => anime.anime_title != *prev_title,
            (Some(_), None) => true,
            _ => self.current_sources.is_none(),
        };

        if should_reprepare && self.library_selected_anime().is_some() {
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
                    .is_some_and(|sn| idx < self.dubbing_choices_for_season(sn).len())
            });
            self.dubbing_list_state.select(self.selected_dubbing_index);
        }
        if prev_mode == AppMode::LibraryEpisode {
            self.selected_episode_index =
                prev_episode.filter(|&idx| idx < self.selected_episode_count());
            self.episode_list_state.select(self.selected_episode_index);
        }

        self.mode = match prev_mode {
            AppMode::LibraryEpisode if self.selected_episode_count() > 0 => AppMode::LibraryEpisode,
            AppMode::LibraryDubbing if self.selected_season_num().is_some() => {
                AppMode::LibraryDubbing
            }
            AppMode::LibrarySeason if self.library_selected_anime().is_some() => {
                AppMode::LibrarySeason
            }
            _ => AppMode::Library,
        };
        self.sync_library_sidebar_selection();
    }

    fn sync_library_sidebar_selection(&mut self) {
        let Some(anime_id) = self.library_selected_anime_id() else {
            self.select_sidebar_subject(None);
            return;
        };

        self.select_sidebar_subject(Some(anime_id));

        if self.current_details.as_ref().map(|details| details.id) != Some(anime_id) {
            self.loading = true;
            self.activity_message = Some("Завантаження метаданих…".to_string());
        }
    }

    /// Оновлює sidebar subject і постер при зміні вибору у списку випусків.
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
        let studio_owner_id = match self.studio_anime_ids.get(j).copied() {
            Some(id) => id,
            None => return,
        };
        let anime_id = studio_owner_id;
        if self.sidebar_subject() == Some(anime_id) {
            return;
        }
        self.select_sidebar_subject(Some(anime_id));
        if self.current_details.is_none() {
            self.loading = true;
            self.activity_message = Some("Завантаження метаданих випуску…".to_string());
        }
    }

    /// Відновлює постер першого TV-члена франшизи при поверненні до SearchList.
    fn restore_representative_poster(&mut self) {
        let representative_id = self
            .canonical_sidebar_subject()
            .or_else(|| self.studio_anime_ids.first().copied())
            .or_else(|| {
                self.selected_result_index
                    .and_then(|i| self.search_results.get(i))
                    .map(|item| item.id)
            });

        self.select_sidebar_subject(representative_id);
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

            let explicit_status = anime_ids
                .iter()
                .filter_map(|anime_id| history.library.get(anime_id))
                .max_by_key(|record| record.updated_at)
                .map(|record| record.status);
            let inferred_status = if seasons
                .iter()
                .all(|season| season.episodes.iter().all(|episode| episode.watched))
            {
                AnimeStatus::Completed
            } else {
                AnimeStatus::Watching
            };
            let status = explicit_status.unwrap_or(inferred_status);
            if status == AnimeStatus::NotAdded {
                return None;
            }

            Some(LibraryAnimeEntry {
                anime_ids,
                anime_title,
                latest_progress,
                seasons,
                status,
            })
        })
        .collect();

    let mut known_ids = items
        .iter()
        .flat_map(|item| item.anime_ids.iter().copied())
        .collect::<HashSet<_>>();
    let mut library_records = history.library.iter().collect::<Vec<_>>();
    library_records.sort_by_key(|(anime_id, record)| (record.updated_at, **anime_id));
    for (&anime_id, record) in library_records {
        if record.status == AnimeStatus::NotAdded || known_ids.contains(&anime_id) {
            continue;
        }
        if let Some(existing) = items
            .iter_mut()
            .find(|item| item.anime_title == record.title)
        {
            existing.anime_ids.push(anime_id);
            existing.anime_ids.sort_unstable();
            existing.anime_ids.dedup();
            existing.status = record.status;
            known_ids.insert(anime_id);
            continue;
        }
        items.push(LibraryAnimeEntry {
            anime_ids: vec![anime_id],
            anime_title: record.title.clone(),
            latest_progress: WatchProgress {
                anime_id,
                anime_title: record.title.clone(),
                season: 1,
                episode: 1,
                studio_name: String::new(),
                timestamp: 0.0,
                duration: 0.0,
                watched: false,
                updated_at: record.updated_at,
            },
            seasons: Vec::new(),
            status: record.status,
        });
        known_ids.insert(anime_id);
    }
    items.sort_by(|a, b| {
        b.latest_progress
            .updated_at
            .cmp(&a.latest_progress.updated_at)
    });
    items
}

fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte_index, _)| byte_index)
}

fn normalize_studio_name(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn dubbing_choices_for_sources(
    sources: &EpisodeSourcesResponse,
    season_num: u32,
) -> Vec<DubbingChoice<'_>> {
    let ashdi = sources
        .ashdi
        .iter()
        .filter(|studio| studio.season_number == season_num)
        .collect::<Vec<_>>();
    let ashdi_names = ashdi
        .iter()
        .map(|studio| normalize_studio_name(&studio.studio_name))
        .collect::<HashSet<_>>();
    ashdi
        .into_iter()
        .map(DubbingChoice::Ashdi)
        .chain(
            sources
                .moonanime
                .iter()
                .filter(move |studio| studio.season_number == season_num)
                .filter(move |studio| {
                    !ashdi_names.contains(&normalize_studio_name(&studio.studio_name))
                })
                .map(DubbingChoice::MoonAnime),
        )
        .collect()
}

fn sidebar_subject_for_release(release: &ReleaseEntry) -> Option<u32> {
    release
        .anihub_id
        .or_else(|| release.anilist_id.map(|id| ANILIST_POSTER_SUBJECT_BIT | id))
}

fn anime_is_fully_watched(anime: &LibraryAnimeEntry) -> bool {
    !anime.seasons.is_empty()
        && anime
            .seasons
            .iter()
            .all(|season| season.episodes.iter().all(|episode| episode.watched))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_search_cursor_maps_to_byte_boundaries() {
        let text = "Наруто";
        assert_eq!(byte_index_for_char(text, 0), 0);
        assert_eq!(byte_index_for_char(text, 1), "Н".len());
        assert_eq!(byte_index_for_char(text, text.chars().count()), text.len());
    }

    #[test]
    fn release_sidebar_subject_keeps_anihub_and_anilist_namespaces_disjoint() {
        let mut release = ReleaseEntry {
            anihub_id: None,
            anilist_id: Some(42),
            title: "Test".to_string(),
            anime_type: "TV".to_string(),
            year: None,
            poster_url: None,
            episodes_count: None,
            available_episodes: None,
            description: None,
            rating: None,
            genres: None,
            dubbing_studios: None,
            conceptual_season: Some(1),
            part: Some(1),
            classification: crate::api::ReleaseClassification::MainlineSeason,
            availability: ReleaseAvailability::Unavailable,
        };

        assert_eq!(sidebar_subject_for_release(&release), Some(0x8000_002a));
        release.anihub_id = Some(7);
        assert_eq!(sidebar_subject_for_release(&release), Some(7));
    }

    #[test]
    fn ashdi_dubbings_precede_only_additional_moonanime_dubbings() {
        let sources = EpisodeSourcesResponse {
            ashdi: vec![
                AshdiStudio {
                    id: 1,
                    studio_name: "Amanogawa".to_string(),
                    season_number: 1,
                    episodes: Vec::new(),
                    episodes_count: 12,
                },
                AshdiStudio {
                    id: 2,
                    studio_name: "Glass Moon".to_string(),
                    season_number: 1,
                    episodes: Vec::new(),
                    episodes_count: 12,
                },
            ],
            moonanime: vec![
                MoonAnimeSourceMarker {
                    studio_name: "Amanogawa".to_string(),
                    season_number: 1,
                    episodes_count: 12,
                    episodes: Vec::new(),
                },
                MoonAnimeSourceMarker {
                    studio_name: "GlassMoon".to_string(),
                    season_number: 1,
                    episodes_count: 12,
                    episodes: Vec::new(),
                },
                MoonAnimeSourceMarker {
                    studio_name: "Dzuski".to_string(),
                    season_number: 1,
                    episodes_count: 10,
                    episodes: Vec::new(),
                },
            ],
        };

        let choices = dubbing_choices_for_sources(&sources, 1);
        assert_eq!(
            choices
                .iter()
                .map(DubbingChoice::studio_name)
                .collect::<Vec<_>>(),
            vec!["Amanogawa", "Glass Moon", "Dzuski"]
        );
        assert!(!choices[0].is_moonanime());
        assert!(choices[2].is_moonanime());
    }

    #[test]
    fn progress_creates_one_library_item() {
        let mut history = AppHistory::default();
        history.progress.insert(
            StorageManager::make_progress_key(7, 1, 2, "Studio"),
            WatchProgress {
                anime_id: 7,
                anime_title: "Тест".to_string(),
                season: 1,
                episode: 2,
                studio_name: "Studio".to_string(),
                timestamp: 120.0,
                duration: 1400.0,
                watched: false,
                updated_at: 1,
            },
        );
        let items = build_library_items(&history);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].anime_title, "Тест");
    }

    #[test]
    fn explicit_library_status_adds_and_removes_unplayed_anime() {
        let mut history = AppHistory::default();
        history.library.insert(
            42,
            crate::storage::history::AnimeLibraryRecord {
                title: "Каґуя".to_string(),
                status: AnimeStatus::Planned,
                updated_at: 10,
            },
        );
        let items = build_library_items(&history);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].anime_title, "Каґуя");
        assert_eq!(items[0].status, AnimeStatus::Planned);

        history.library.get_mut(&42).unwrap().status = AnimeStatus::NotAdded;
        assert!(build_library_items(&history).is_empty());
    }
}
