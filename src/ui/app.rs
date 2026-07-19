use crate::api::{
    AniListMedia, AnimeDetails, AnimeItem, ApiClient, AshdiStudio, EpisodeSourcesKey,
    EpisodeSourcesResponse, FranchiseCatalog, MoonAnimeBrowserEpisode, MoonAnimeSourceMarker,
    ReleaseAvailability, ReleaseClassification, ReleaseEntry,
};
use crate::cache::MetadataCache;
use crate::poster_cache::PosterCache;
use crate::settings::{
    DefaultLibraryFilter, GITHUB_URL, LibrarySortPreference, SearchSortPreference, Settings,
    SettingsStore, StartScreen, ThemePreset, UpdateCheck, mpv_is_available,
};
use crate::storage::{
    AnimeStatus, AnimeStatusUpdate, AppHistory, EpisodeWatchedUpdate, LibraryReleaseMetadata,
    StorageManager,
};
#[cfg(test)]
use crate::storage::{LibraryReleaseKind, WatchProgress};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use image::DynamicImage;
use ratatui::widgets::ListState;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

mod content_ui;
mod input;
mod library_actions;
mod library_catalog;
mod library_navigation;
mod library_projection;
mod library_sources;
mod library_state;
mod library_sync;
mod playback_ui;
mod search_actions;
mod settings_ui;

use content_ui::ContentUiState;
use library_catalog::*;
use library_projection::*;
pub use library_state::{
    LibraryAnimeEntry, LibraryFilter, LibrarySeasonEntry, LibrarySort, LibraryState,
    LibraryWatchedConfirmation,
};
use playback_ui::PlaybackUiState;
use settings_ui::SettingsUiState;

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
pub enum SearchSort {
    Relevance,
    Title,
    Year,
    Rating,
}

impl SearchSort {
    pub const ALL: [Self; 4] = [Self::Relevance, Self::Title, Self::Year, Self::Rating];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Relevance => "Збіг",
            Self::Title => "Назва",
            Self::Year => "Рік",
            Self::Rating => "Рейтинг",
        }
    }

    pub const fn order_label(self, reversed: bool) -> &'static str {
        match (self, reversed) {
            (Self::Relevance, false) => "кращі → слабші",
            (Self::Relevance, true) => "слабші → кращі",
            (Self::Title, false) => "А → Я",
            (Self::Title, true) => "Я → А",
            (Self::Year, false) => "новіші → старіші",
            (Self::Year, true) => "старіші → новіші",
            (Self::Rating, false) => "вищий → нижчий",
            (Self::Rating, true) => "нижчий → вищий",
        }
    }

    pub const fn direction_symbol(self, reversed: bool) -> &'static str {
        let ascending = matches!(self, Self::Title) != reversed;
        if ascending { "↑" } else { "↓" }
    }
}

pub struct SearchOrderingState {
    pub sort: SearchSort,
    pub reversed: bool,
    pub popup: Option<usize>,
}

impl Default for SearchOrderingState {
    fn default() -> Self {
        Self {
            sort: SearchSort::Relevance,
            reversed: false,
            popup: None,
        }
    }
}

impl SearchOrderingState {
    fn new(sort: SearchSort, reversed: bool) -> Self {
        Self {
            sort,
            reversed,
            popup: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimeStatusEditor {
    pub anime_ids: Vec<u32>,
    pub releases: Vec<Option<LibraryReleaseMetadata>>,
    pub title: String,
    pub selected: usize,
}

type AnimeStatusContext = (
    Vec<u32>,
    Vec<Option<LibraryReleaseMetadata>>,
    String,
    AnimeStatus,
);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsTab {
    #[default]
    General,
    Themes,
    About,
}

impl SettingsTab {
    pub const ALL: [Self; 3] = [Self::General, Self::Themes, Self::About];

    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "Основні",
            Self::Themes => "Теми",
            Self::About => "Про",
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[index.checked_sub(1).unwrap_or(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsInput {
    MpvPath,
    MpvArgs,
}

/// Multi-choice setting edited through a centered radio popup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsChoiceKind {
    StartScreen,
    LibraryFilter,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsChoiceEditor {
    pub kind: SettingsChoiceKind,
    pub selected: usize,
}

/// Draft state for the watched-threshold slider popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsThresholdEditor {
    /// `None` means the auto-watched feature is disabled (Space).
    pub percent: Option<u8>,
}

impl SettingsChoiceKind {
    pub const fn title(self) -> &'static str {
        match self {
            Self::StartScreen => " Стартовий екран ",
            Self::LibraryFilter => " Фільтр бібліотеки ",
        }
    }

    pub fn option_labels(self) -> Vec<&'static str> {
        match self {
            Self::StartScreen => vec![StartScreen::Search.label(), StartScreen::Library.label()],
            Self::LibraryFilter => DefaultLibraryFilter::ALL
                .iter()
                .map(|filter| filter.label())
                .collect(),
        }
    }

    pub fn selected_index(self, settings: &Settings) -> usize {
        match self {
            Self::StartScreen => match settings.start_screen {
                StartScreen::Search => 0,
                StartScreen::Library => 1,
            },
            Self::LibraryFilter => DefaultLibraryFilter::ALL
                .iter()
                .position(|filter| *filter == settings.default_library_filter)
                .unwrap_or(0),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Current(String),
    Available(UpdateCheck),
    Failed(String),
}

/// Watched-threshold slider: 50–100% in steps of 5.
pub const THRESHOLD_MIN: u8 = 50;
pub const THRESHOLD_MAX: u8 = 100;
pub const THRESHOLD_STEP: u8 = 5;
pub const THRESHOLD_BAR_WIDTH: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub anime_id: u32,
    pub anime_title: String,
    pub season: u32,
    pub episode: u32,
    pub studio_name: String,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
}

pub type HistoryIndexes = (HashSet<(u32, u32, u32)>, HashMap<(u32, u32, u32), f64>);
const ANILIST_POSTER_SUBJECT_BIT: u32 = 0x8000_0000;

#[derive(Clone, Copy)]
pub enum DubbingChoice<'a> {
    Ashdi(&'a AshdiStudio, Option<&'a str>),
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
        canonical_studio_name(match self {
            Self::Ashdi(studio, display_alias) => display_alias.unwrap_or(&studio.studio_name),
            Self::MoonAnime(studio) => &studio.studio_name,
        })
    }

    pub fn episodes_count(&self) -> u32 {
        match self {
            Self::Ashdi(studio, _) => studio.episodes_count,
            Self::MoonAnime(studio) => studio.episodes_count,
        }
    }

    pub fn is_moonanime(&self) -> bool {
        matches!(self, Self::MoonAnime(_))
    }
}

/// Search-tab state kept together so result projection, selection ownership,
/// and text editing do not keep growing the application-wide state bag.
pub struct SearchState {
    pub query: String,
    pub last_query: String,
    /// Cursor position in Unicode scalar values, not bytes.
    pub cursor: usize,
    pub results: Vec<AnimeItem>,
    pub ordering: SearchOrderingState,
    pub franchise_groups: Vec<Vec<usize>>,
    /// Release catalogs aligned with `franchise_groups` by index.
    pub franchise_catalogs: Vec<FranchiseCatalog>,
    /// Relation metadata retained so catalogs can be rebuilt after an AniHub
    /// availability lookup completes.
    pub anilist_media: Vec<AniListMedia>,
    pub selected_group_index: Option<usize>,
    pub selected_result_index: Option<usize>,
    pub selected_release_index: Option<usize>,
    pub result_list_state: ListState,
}

impl SearchState {
    fn new(ordering: SearchOrderingState, franchise_catalogs: Vec<FranchiseCatalog>) -> Self {
        Self {
            query: String::new(),
            last_query: String::new(),
            cursor: 0,
            results: Vec::new(),
            ordering,
            franchise_groups: Vec::new(),
            franchise_catalogs,
            anilist_media: Vec::new(),
            selected_group_index: None,
            selected_result_index: None,
            selected_release_index: None,
            result_list_state: ListState::default(),
        }
    }
}

pub struct AppState {
    pub mode: AppMode,
    pub focus: FocusPanel,
    pub search: SearchState,
    pub content: ContentUiState,

    pub library: LibraryState,
    pub status_editor: Option<AnimeStatusEditor>,

    pub settings: Settings,
    pub(crate) settings_ui: SettingsUiState,
    pub settings_store: SettingsStore,
    pub metadata_cache: MetadataCache,

    pub should_quit: bool,
    pub api_client: ApiClient,
    pub storage: StorageManager,
    pub history: AppHistory,
    pub loading: bool,
    pub activity_message: Option<String>,
    pub(crate) playback: PlaybackUiState,
    pub status_message: Option<(String, StatusKind)>,
    pub status_expires_at: Option<Instant>,
    pub status_retry_available: bool,
    pub retry_requested: bool,
    pub show_help: bool,
    /// (display title, direct MoonAnime iframe URL)
    pub moonanime_browser_prompt: Option<(String, String)>,

    // Кеші ресурсів
    pub details_cache: moka::sync::Cache<u32, AnimeDetails>,
    pub sources_cache: moka::sync::Cache<EpisodeSourcesKey, EpisodeSourcesResponse>,

    // Обкладинка
    pub picker: Picker,
    pub current_poster: Option<StatefulProtocol>,
    pub poster_cache: moka::sync::Cache<u32, std::sync::Arc<DynamicImage>>,
    pub poster_disk_cache: PosterCache,
    pub poster_fetch_pending: Option<u32>,

    // O(1) індекси для перевірки переглянутих серій під час рендеру.
    // Ребілдяться щоразу коли змінюється `history`.
    /// (anime_id, season, episode) → true якщо watched
    pub watched_index: HashSet<(u32, u32, u32)>,
    /// (anime_id, season, episode) → timestamp якщо в процесі перегляду (не watched, >= 10s)
    pub progress_index: HashMap<(u32, u32, u32), f64>,
}

impl AppState {
    pub fn new(picker: Picker, image_protocol: impl Into<String>) -> anyhow::Result<Self> {
        let storage = StorageManager::new()?;
        let history = storage.load_history()?;
        let (watched_index, progress_index) = Self::build_history_indexes(&history);
        let settings_store = SettingsStore::new()?;
        let mut settings = settings_store.load()?;
        let initialized_new_content = settings.initialize_new_content_tracking(
            history.library.iter().map(|(&anime_id, record)| {
                (
                    anime_id,
                    record
                        .release
                        .as_ref()
                        .and_then(|release| release.episodes_count),
                )
            }),
        );
        if initialized_new_content {
            settings_store.save(&settings)?;
        }
        let metadata_cache = MetadataCache::new(settings_store.data_dir())?;
        let cached_library_catalogs =
            cached_franchise_catalogs_for_history(&metadata_cache, &history);
        let poster_disk_cache = PosterCache::new(settings_store.data_dir())?;
        let details_cache = moka::sync::Cache::builder().max_capacity(500).build();
        for (anime_id, details) in metadata_cache.details() {
            details_cache.insert(anime_id, details);
        }
        let mpv_available = mpv_is_available(&settings.mpv_path);
        let default_library_filter = library_filter_from_setting(
            settings
                .last_library_filter
                .unwrap_or(settings.default_library_filter),
        );
        let library_sort = library_sort_from_setting(settings.library_sort);
        let search_sort = search_sort_from_setting(settings.search_sort);
        let start_in_library = settings.start_screen == StartScreen::Library;

        let mut app = Self {
            mode: AppMode::Normal,
            focus: FocusPanel::SearchList,
            // Cached searches retain AniList relation graphs. Keeping the
            // catalogs that intersect history lets the first Library open
            // restore sibling seasons before another network search.
            search: SearchState::new(
                SearchOrderingState::new(search_sort, settings.search_sort_reversed),
                cached_library_catalogs,
            ),
            content: ContentUiState::default(),

            library: LibraryState::new(
                default_library_filter,
                library_sort,
                settings.library_sort_reversed,
            ),
            status_editor: None,

            settings,
            settings_ui: SettingsUiState::new(mpv_available, image_protocol.into()),
            settings_store,
            metadata_cache,

            should_quit: false,
            api_client: ApiClient::new()?,
            storage,
            history,
            loading: false,
            activity_message: None,
            playback: PlaybackUiState::default(),
            status_message: None,
            status_expires_at: None,
            status_retry_available: false,
            retry_requested: false,
            show_help: false,
            moonanime_browser_prompt: None,

            details_cache,
            sources_cache: moka::sync::Cache::builder().max_capacity(100).build(),

            picker,
            current_poster: None,
            poster_cache: moka::sync::Cache::builder().max_capacity(30).build(),
            poster_disk_cache,
            poster_fetch_pending: None,

            watched_index,
            progress_index,
        };
        if start_in_library {
            app.open_library();
        }
        Ok(app)
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
        self.content
            .sidebar_subject_id
            .or_else(|| {
                self.content
                    .sidebar_anime_idx
                    .and_then(|index| self.search.results.get(index))
                    .map(|anime| anime.id)
            })
            .or_else(|| {
                self.search
                    .selected_result_index
                    .and_then(|index| self.search.results.get(index))
                    .map(|anime| anime.id)
            })
    }

    /// Select the release whose metadata and poster should appear in the
    /// sidebar. This is the single state transition used by release browsing.
    pub fn select_sidebar_subject(&mut self, anime_id: Option<u32>) {
        let subject_changed = self.content.sidebar_subject_id != anime_id;
        self.content.sidebar_subject_id = anime_id;
        self.content.sidebar_anime_idx =
            anime_id.and_then(|id| self.search.results.iter().position(|anime| anime.id == id));
        self.content.current_details = anime_id.and_then(|id| self.details_cache.get(&id));

        if !self.settings.show_posters {
            self.current_poster = None;
            self.poster_fetch_pending = None;
            return;
        }

        match anime_id {
            Some(id) => {
                if !subject_changed
                    && (self.current_poster.is_some() || self.poster_fetch_pending == Some(id))
                {
                    return;
                }
                self.current_poster = None;
                self.poster_fetch_pending = Some(id);
            }
            None => {
                self.current_poster = None;
                self.poster_fetch_pending = None;
            }
        }
    }

    /// Whether an asynchronously completed poster still owns the sidebar.
    pub fn accepts_poster(&self, anime_id: u32) -> bool {
        self.settings.show_posters && self.sidebar_subject() == Some(anime_id)
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
        self.search
            .selected_group_index
            .and_then(|index| self.search.franchise_catalogs.get(index))
    }

    pub fn has_release_catalog(&self) -> bool {
        self.selected_franchise_catalog()
            .is_some_and(|catalog| !catalog.releases.is_empty())
    }

    pub fn selected_release(&self) -> Option<&ReleaseEntry> {
        let index = self.search.selected_release_index?;
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
            .search
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
        self.search.selected_release_index = index;
        self.content.season_list_state.select(index);
        self.content.selected_dubbing_index = None;
        self.content.dubbing_list_state.select(None);
        self.content.selected_episode_index = None;
        self.content.episode_list_state.select(None);

        let source_key = self.selected_release_source_key();
        let anime_id = source_key.map(|key| key.anime_id);
        let sidebar_subject = self.selected_release_sidebar_subject();
        self.content.current_sources = source_key.and_then(|key| self.sources_cache.get(&key));
        self.content.current_sources_key = self.content.current_sources.as_ref().and(source_key);
        self.content.studio_anime_ids =
            self.content
                .current_sources
                .as_ref()
                .map_or_else(Vec::new, |sources| {
                    vec![
                        anime_id.expect("cached release sources have an owner");
                        sources.ashdi.len()
                    ]
                });
        self.select_sidebar_subject(sidebar_subject);
        match (anime_id, self.content.current_sources.is_some()) {
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
        self.select_release(self.search.selected_release_index);
    }

    // --- Хелпери для 4-панельної навігації ---

    /// Унікальні season_number з current_sources, відсортовані за зростанням.
    pub fn unique_seasons(&self) -> Vec<u32> {
        let Some(sources) = &self.content.current_sources else {
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
        let Some(sources) = &self.content.current_sources else {
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
        let Some(sources) = &self.content.current_sources else {
            return Vec::new();
        };
        dubbing_choices_for_sources(sources, season_num)
    }

    /// Поточний season_number за selected_season_index.
    pub fn selected_season_num(&self) -> Option<u32> {
        if self.is_library_mode() {
            let idx = self.content.selected_season_index?;
            self.library_selected_anime()?
                .seasons
                .get(idx)
                .map(|release| release.season)
        } else if self.has_release_catalog() {
            (self.selected_release_available()
                && self.content.current_sources_key == self.selected_release_source_key())
            .then(|| self.unique_seasons().into_iter().next())
            .flatten()
        } else {
            let idx = self.content.selected_season_index?;
            self.unique_seasons().get(idx).copied()
        }
    }

    /// Обрана студія (для відтворення та списку епізодів).
    pub fn selected_studio(&self) -> Option<&AshdiStudio> {
        match self.selected_dubbing_choice()? {
            DubbingChoice::Ashdi(studio, _) => Some(studio),
            DubbingChoice::MoonAnime(_) => None,
        }
    }

    pub fn selected_dubbing_choice(&self) -> Option<DubbingChoice<'_>> {
        let season_num = self.selected_season_num()?;
        let dub_idx = self.content.selected_dubbing_index?;
        self.dubbing_choices_for_season(season_num)
            .into_iter()
            .nth(dub_idx)
    }

    pub fn selected_episode_choices(&self) -> Vec<EpisodeChoice<'_>> {
        match self.selected_dubbing_choice() {
            Some(DubbingChoice::Ashdi(studio, _)) => {
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

    pub fn selected_episode_choice(&self) -> Option<EpisodeChoice<'_>> {
        let index = self.content.selected_episode_index?;
        self.selected_episode_choices().get(index).copied()
    }

    fn selected_dubbing_is_moonanime(&self) -> bool {
        let Some(season) = self.selected_season_num() else {
            return false;
        };
        let Some(index) = self.content.selected_dubbing_index else {
            return false;
        };
        self.dubbing_choices_for_season(season)
            .get(index)
            .is_some_and(DubbingChoice::is_moonanime)
    }

    pub fn library_selected_anime(&self) -> Option<&LibraryAnimeEntry> {
        self.library
            .anime_index
            .and_then(|idx| self.library.items.get(idx))
    }

    pub fn library_season_numbers(&self) -> Vec<u32> {
        let Some(anime) = self.library_selected_anime() else {
            return Vec::new();
        };
        anime.seasons.iter().map(|release| release.season).collect()
    }

    pub fn library_selected_season(&self) -> Option<&LibrarySeasonEntry> {
        let anime = self.library_selected_anime()?;
        self.content
            .selected_season_index
            .and_then(|idx| anime.seasons.get(idx))
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
        self.search.ordering.popup = None;
        self.library.sort_popup = None;
        self.library.pending_watched_confirmation = None;
        self.library.pending_delete_confirmation = None;
        self.library.clear_confirmation = false;
        self.moonanime_browser_prompt = None;
        self.library.search_editing = false;
        self.settings_ui.close_editors();
        // Leaving search-edit must not require Esc first when the user
        // deliberately picks another primary tab (including via Alt/Ctrl).
        if self.mode == AppMode::SearchInput {
            self.search.query.clear();
            self.search.cursor = 0;
        }
        match tab {
            // Always land on Search in Normal mode (never auto-open the editor).
            // `/` is the only way to start typing a query.
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
        // While typing a query, bare 1/2/3 must remain insertable. Tab switches
        // still work with Alt or Ctrl so the user is never trapped in the editor.
        let chord = modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if (self.mode == AppMode::SearchInput || self.library.search_editing) && !chord {
            return false;
        }
        match code {
            KeyCode::Char('1') => self.switch_primary_tab(PrimaryTab::Search),
            KeyCode::Char('2') => self.switch_primary_tab(PrimaryTab::Library),
            KeyCode::Char('3') => self.switch_primary_tab(PrimaryTab::Settings),
            _ => return false,
        }
        true
    }

    pub fn take_update_check_request(&mut self) -> bool {
        std::mem::take(&mut self.settings_ui.update_check_requested)
    }

    pub fn take_discord_config_changed(&mut self) -> bool {
        std::mem::take(&mut self.settings_ui.discord_config_changed)
    }

    pub fn finish_update_check(&mut self, result: anyhow::Result<UpdateCheck>) {
        self.settings_ui.update_state = match result {
            Ok(update) if update.update_available => UpdateState::Available(update),
            Ok(update) => UpdateState::Current(update.latest_version),
            Err(error) => UpdateState::Failed(error.to_string()),
        };
        // Keep the update popup open so the user sees the result in-dialog.
        self.clear_info_status();
    }

    // ---

    pub fn handle_events(&mut self) -> anyhow::Result<()> {
        self.clear_expired_status();

        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Layout-independent shortcut key (ЙЦУКЕН → QWERTY). Typed
                    // text in search/settings keeps the raw character below.
                    let shortcut = super::keys::shortcut_code(key.code);
                    let raw = key.code;

                    if shortcut == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        self.should_quit = true;
                        return Ok(());
                    }
                    if matches!(self.status_message, Some((_, StatusKind::Error))) {
                        match shortcut {
                            KeyCode::Char('r') if self.status_retry_available => {
                                self.clear_status();
                                self.retry_requested = true;
                                self.set_activity("Повторна спроба…");
                            }
                            KeyCode::Esc | KeyCode::Enter => self.clear_status(),
                            _ => {}
                        }
                        return Ok(());
                    }
                    if self.show_help {
                        self.show_help = false;
                        return Ok(());
                    }

                    if self.handle_moonanime_browser_prompt(shortcut) {
                        return Ok(());
                    }

                    if self.handle_status_editor(shortcut) {
                        return Ok(());
                    }

                    if self.handle_search_sort_popup(shortcut) {
                        return Ok(());
                    }

                    if self.handle_library_sort_popup(shortcut) {
                        return Ok(());
                    }

                    if self.handle_library_watched_confirmation(shortcut) {
                        return Ok(());
                    }

                    if self.handle_pending_delete_confirmation(shortcut) {
                        return Ok(());
                    }

                    if self.handle_clear_library_confirmation(shortcut) {
                        return Ok(());
                    }

                    if self.handle_settings_update_popup(shortcut) {
                        return Ok(());
                    }

                    if self.handle_settings_threshold(shortcut) {
                        return Ok(());
                    }

                    if self.handle_settings_choice(shortcut) {
                        return Ok(());
                    }

                    // Path/args editors must receive the raw glyph so non-Latin
                    // paths and arguments can still be typed.
                    if self.handle_settings_input(raw) {
                        return Ok(());
                    }

                    if self.handle_primary_tab_key(shortcut, key.modifiers) {
                        return Ok(());
                    }
                    if self.library.search_editing {
                        self.handle_library_search_key(raw);
                        return Ok(());
                    }
                    self.clear_info_status();

                    if self.mode != AppMode::SearchInput
                        && (shortcut == KeyCode::Char('?') || shortcut == KeyCode::Char('h'))
                    {
                        self.show_help = true;
                        return Ok(());
                    }

                    if !matches!(self.mode, AppMode::SearchInput | AppMode::Settings)
                        && self.handle_list_navigation_key(shortcut)
                    {
                        return Ok(());
                    }

                    input::handle_mode_key(self, shortcut, raw);
                }
            }
        }
        Ok(())
    }

    fn insert_search_char(&mut self, character: char) {
        let byte_index = byte_index_for_char(&self.search.query, self.search.cursor);
        self.search.query.insert(byte_index, character);
        self.search.cursor += 1;
    }

    fn backspace_search_char(&mut self) {
        if self.search.cursor == 0 {
            return;
        }
        let start = byte_index_for_char(&self.search.query, self.search.cursor - 1);
        let end = byte_index_for_char(&self.search.query, self.search.cursor);
        self.search.query.replace_range(start..end, "");
        self.search.cursor -= 1;
    }

    fn delete_search_char(&mut self) {
        let char_count = self.search.query.chars().count();
        if self.search.cursor >= char_count {
            return;
        }
        let start = byte_index_for_char(&self.search.query, self.search.cursor);
        let end = byte_index_for_char(&self.search.query, self.search.cursor + 1);
        self.search.query.replace_range(start..end, "");
    }

    fn handle_library_search_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => self.library.search_editing = false,
            KeyCode::Esc => {
                self.library.search_editing = false;
                self.library.search_query.clear();
                self.library.search_cursor = 0;
                self.apply_library_filter();
            }
            KeyCode::Tab => self.cycle_library_filter(false),
            KeyCode::BackTab => self.cycle_library_filter(true),
            KeyCode::Char(character) => {
                let byte_index =
                    byte_index_for_char(&self.library.search_query, self.library.search_cursor);
                self.library.search_query.insert(byte_index, character);
                self.library.search_cursor += 1;
                self.apply_library_filter();
            }
            KeyCode::Backspace if self.library.search_cursor > 0 => {
                let start =
                    byte_index_for_char(&self.library.search_query, self.library.search_cursor - 1);
                let end =
                    byte_index_for_char(&self.library.search_query, self.library.search_cursor);
                self.library.search_query.replace_range(start..end, "");
                self.library.search_cursor -= 1;
                self.apply_library_filter();
            }
            KeyCode::Delete
                if self.library.search_cursor < self.library.search_query.chars().count() =>
            {
                let start =
                    byte_index_for_char(&self.library.search_query, self.library.search_cursor);
                let end =
                    byte_index_for_char(&self.library.search_query, self.library.search_cursor + 1);
                self.library.search_query.replace_range(start..end, "");
                self.apply_library_filter();
            }
            KeyCode::Left => {
                self.library.search_cursor = self.library.search_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                self.library.search_cursor =
                    (self.library.search_cursor + 1).min(self.library.search_query.chars().count());
            }
            KeyCode::Home => self.library.search_cursor = 0,
            KeyCode::End => {
                self.library.search_cursor = self.library.search_query.chars().count();
            }
            _ => {}
        }
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
                    self.library.anime_list_state.selected().unwrap_or(0),
                    self.library.items.len(),
                ),
                AppMode::LibrarySeason => (
                    self.content.season_list_state.selected().unwrap_or(0),
                    self.library_season_numbers().len(),
                ),
                AppMode::LibraryDubbing => {
                    let total = self
                        .selected_season_num()
                        .map_or(0, |season| self.dubbing_choices_for_season(season).len());
                    (
                        self.content.dubbing_list_state.selected().unwrap_or(0),
                        total,
                    )
                }
                AppMode::LibraryEpisode => (
                    self.content.episode_list_state.selected().unwrap_or(0),
                    self.selected_episode_count(),
                ),
                _ => (0, 0),
            };
        }

        match self.focus {
            FocusPanel::SearchList => (
                self.search.result_list_state.selected().unwrap_or(0),
                self.search.franchise_groups.len(),
            ),
            FocusPanel::ReleaseList => (
                self.content.season_list_state.selected().unwrap_or(0),
                self.release_count(),
            ),
            FocusPanel::DubbingList => {
                let total = self
                    .selected_season_num()
                    .map_or(0, |season| self.dubbing_choices_for_season(season).len());
                (
                    self.content.dubbing_list_state.selected().unwrap_or(0),
                    total,
                )
            }
            FocusPanel::EpisodeList => (
                self.content.episode_list_state.selected().unwrap_or(0),
                self.selected_episode_count(),
            ),
        }
    }

    fn cycle_library_filter(&mut self, backwards: bool) {
        let filter = if backwards {
            self.library.filter.previous()
        } else {
            self.library.filter.next()
        };
        self.set_library_filter(filter);
    }

    fn set_library_filter(&mut self, filter: LibraryFilter) {
        self.library.filter = filter;
        self.settings.last_library_filter = Some(library_filter_to_setting(filter));
        self.mode = AppMode::Library;
        self.apply_library_filter();
    }

    fn apply_library_filter(&mut self) {
        let selected_ids = self
            .library_selected_anime()
            .map(|anime| anime.anime_ids.clone());
        self.library.items = self
            .library
            .all_items
            .iter()
            .filter(|anime| {
                library_item_matches(anime, self.library.filter, &self.library.search_query)
            })
            .cloned()
            .collect();
        sort_library_items(
            &mut self.library.items,
            self.library.sort,
            self.library.sort_reversed,
            &self.details_cache,
            &self.watched_index,
        );
        self.library.anime_index = selected_ids
            .and_then(|ids| {
                self.library
                    .items
                    .iter()
                    .position(|anime| anime.anime_ids.iter().any(|id| ids.contains(id)))
            })
            .or_else(|| {
                self.settings
                    .last_library_anime_id
                    .and_then(|remembered_id| {
                        self.library
                            .items
                            .iter()
                            .position(|anime| anime.anime_ids.contains(&remembered_id))
                    })
            })
            .or_else(|| (!self.library.items.is_empty()).then_some(0));
        self.library
            .anime_list_state
            .select(self.library.anime_index);
        self.content.season_list_state.select(None);
        self.content.dubbing_list_state.select(None);
        self.content.episode_list_state.select(None);
        self.library.pending_delete_confirmation = None;
        if self.library.items.is_empty() {
            self.content.current_sources = None;
            self.content.current_sources_key = None;
            self.content.current_details = None;
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
            let anime_id = self.library_selected_anime_id()?;

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
                if let Some(item) = self.search.results.iter().find(|i| i.id == id) {
                    return Some((id, item.title_ukrainian.clone(), item.slug.clone()));
                }
            }

            // Priority 2: Sidebar/Selected result index
            let idx = self
                .content
                .sidebar_anime_idx
                .or(self.search.selected_result_index)?;
            let item = self.search.results.get(idx)?;
            Some((item.id, item.title_ukrainian.clone(), item.slug.clone()))
        }
    }

    fn selected_anime_status_context(&self) -> Option<AnimeStatusContext> {
        let (mut targets, title) = if self.is_library_mode() {
            let anime = self.library_selected_anime()?;
            if self.mode != AppMode::Library {
                let release = self.library_selected_season()?;
                let status = self
                    .history
                    .library
                    .get(&release.anime_id)
                    .map_or(release.status, |record| record.status);
                return Some((
                    vec![release.anime_id],
                    vec![Some(release.metadata())],
                    anime.anime_title.clone(),
                    status,
                ));
            }
            (
                anime
                    .anime_ids
                    .iter()
                    .map(|anime_id| {
                        let release = anime
                            .seasons
                            .iter()
                            .find(|release| release.anime_id == *anime_id)
                            .map(LibrarySeasonEntry::metadata)
                            .or_else(|| {
                                self.history
                                    .library
                                    .get(anime_id)
                                    .and_then(|record| record.release.clone())
                            });
                        (*anime_id, release)
                    })
                    .collect::<Vec<_>>(),
                anime.anime_title.clone(),
            )
        } else {
            let group_index = self.search.selected_group_index?;
            if let Some(catalog) = self.search.franchise_catalogs.get(group_index) {
                if self.focus != FocusPanel::SearchList {
                    let release = self.selected_release()?;
                    let anime_id = self.selected_release_anihub_id()?;
                    (
                        vec![(
                            anime_id,
                            Some(library_metadata_for_release(catalog, release)),
                        )],
                        catalog.canonical_title.clone(),
                    )
                } else {
                    (
                        catalog
                            .releases
                            .iter()
                            .filter(|release| {
                                release.availability == ReleaseAvailability::Available
                            })
                            .filter_map(|release| {
                                release.anihub_id.map(|anime_id| {
                                    (
                                        anime_id,
                                        Some(library_metadata_for_release(catalog, release)),
                                    )
                                })
                            })
                            .collect::<Vec<_>>(),
                        catalog.canonical_title.clone(),
                    )
                }
            } else {
                let group = self.search.franchise_groups.get(group_index)?;
                let representative = group
                    .first()
                    .and_then(|index| self.search.results.get(*index))?;
                (
                    group
                        .iter()
                        .filter_map(|index| {
                            self.search
                                .results
                                .get(*index)
                                .map(|anime| (anime.id, None))
                        })
                        .collect::<Vec<_>>(),
                    representative.title_ukrainian.clone(),
                )
            }
        };
        targets.sort_by_key(|(anime_id, _)| *anime_id);
        targets.dedup_by_key(|(anime_id, _)| *anime_id);
        let (anime_ids, releases): (Vec<_>, Vec<_>) = targets.into_iter().unzip();
        if anime_ids.is_empty() {
            return None;
        }

        let explicit = anime_ids
            .iter()
            .filter_map(|anime_id| self.history.library.get(anime_id))
            .max_by_key(|record| record.updated_at)
            .map(|record| record.status);
        let status = explicit.unwrap_or_else(|| {
            let has_progress = self
                .history
                .progress
                .values()
                .any(|progress| anime_ids.contains(&progress.anime_id));
            if has_progress {
                AnimeStatus::Watching
            } else {
                AnimeStatus::NotAdded
            }
        });
        Some((anime_ids, releases, title, status))
    }

    fn open_status_editor(&mut self) {
        let Some((anime_ids, releases, title, status)) = self.selected_anime_status_context()
        else {
            return;
        };
        let selected = AnimeStatus::ALL
            .iter()
            .position(|candidate| *candidate == status)
            .unwrap_or(0);
        self.status_editor = Some(AnimeStatusEditor {
            anime_ids,
            releases,
            title,
            selected,
        });
    }

    fn completed_episode_updates(
        &self,
        anime_ids: &[u32],
        anime_title: &str,
    ) -> Vec<EpisodeWatchedUpdate> {
        let mut source_keys = self
            .search
            .franchise_catalogs
            .iter()
            .flat_map(|catalog| catalog.releases.iter())
            .filter(|release| release.availability == ReleaseAvailability::Available)
            .filter_map(|release| {
                let anime_id = release.anihub_id?;
                anime_ids
                    .contains(&anime_id)
                    .then_some(EpisodeSourcesKey::new(
                        anime_id,
                        release.conceptual_season.unwrap_or(1),
                    ))
            })
            .collect::<Vec<_>>();
        if source_keys.is_empty() {
            source_keys.extend(
                anime_ids
                    .iter()
                    .map(|anime_id| self.source_key_for_anime_id(*anime_id)),
            );
        }
        source_keys.sort_by_key(|key| (key.anime_id, key.season));
        source_keys.dedup();

        let mut seen = HashSet::new();
        let mut updates = Vec::new();
        for source_key in source_keys {
            if let Some(sources) = self.sources_cache.get(&source_key) {
                for studio in &sources.ashdi {
                    for episode in &studio.episodes {
                        push_completed_episode(
                            &mut updates,
                            &mut seen,
                            source_key.anime_id,
                            anime_title,
                            studio.season_number,
                            episode.episode_number,
                            &studio.studio_name,
                        );
                    }
                }
                for studio in &sources.moonanime {
                    for episode in &studio.episodes {
                        push_completed_episode(
                            &mut updates,
                            &mut seen,
                            source_key.anime_id,
                            anime_title,
                            studio.season_number,
                            episode.episode_number,
                            &studio.studio_name,
                        );
                    }
                }
            }

            if seen.iter().any(|(anime_id, season, _)| {
                *anime_id == source_key.anime_id && *season == source_key.season
            }) {
                continue;
            }

            let fallback = self.search.franchise_catalogs.iter().find_map(|catalog| {
                let release = catalog.releases.iter().find(|release| {
                    release.anihub_id == Some(source_key.anime_id)
                        && release.conceptual_season.unwrap_or(1) == source_key.season
                })?;
                let count = release.available_episodes.or(release.episodes_count)?;
                let part = release.part.unwrap_or(1);
                let offset = catalog
                    .releases
                    .iter()
                    .filter(|candidate| {
                        candidate.classification == release.classification
                            && candidate.conceptual_season == release.conceptual_season
                            && candidate.part.unwrap_or(1) < part
                    })
                    .filter_map(|candidate| {
                        candidate.available_episodes.or(candidate.episodes_count)
                    })
                    .sum::<u32>();
                Some((release.conceptual_season.unwrap_or(1), offset + 1, count))
            });
            let fallback = fallback.or_else(|| {
                self.details_cache
                    .get(&source_key.anime_id)
                    .or_else(|| {
                        self.content
                            .current_details
                            .as_ref()
                            .filter(|details| details.id == source_key.anime_id)
                            .cloned()
                    })
                    .and_then(|details| {
                        details
                            .episodes_count
                            .map(|count| (source_key.season, 1, count))
                    })
            });
            if let Some((season, first_episode, count)) = fallback {
                for episode in first_episode..first_episode.saturating_add(count) {
                    push_completed_episode(
                        &mut updates,
                        &mut seen,
                        source_key.anime_id,
                        anime_title,
                        season,
                        episode,
                        "Статус",
                    );
                }
            }
        }
        updates
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
                if status == AnimeStatus::Completed {
                    let updates = self.completed_episode_updates(&editor.anime_ids, &editor.title);
                    if !updates.is_empty() {
                        match self.storage.set_episodes_watched(&updates) {
                            Ok(history) => self.history = history,
                            Err(error) => {
                                self.set_error_status(format!(
                                    "Не вдалося позначити всі серії: {error}"
                                ));
                                return true;
                            }
                        }
                    }
                }
                let status_updates = editor
                    .anime_ids
                    .iter()
                    .copied()
                    .zip(editor.releases.iter().cloned())
                    .map(|(anime_id, release)| AnimeStatusUpdate {
                        anime_id,
                        title: editor.title.clone(),
                        status,
                        release,
                    })
                    .collect::<Vec<_>>();
                match self.storage.set_anime_statuses(&status_updates) {
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
        let Some(index) = self.content.selected_episode_index else {
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
            .content
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
            self.playback.play_requested = true;
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
                            .is_some_and(|s| !s.ashdi.is_empty());
                        let seasons = self.unique_seasons();
                        if has_seasons && !seasons.is_empty() {
                            self.content.selected_season_index = Some(0);
                            self.content.season_list_state.select(Some(0));
                            self.update_sidebar_for_season();
                            if seasons.len() == 1 {
                                let season_num = seasons[0];
                                let studios_len = self.dubbing_choices_for_season(season_num).len();
                                if studios_len > 0 {
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
                } else if let Some(sn) = self.selected_season_num() {
                    let studios_len = self.dubbing_choices_for_season(sn).len();
                    if studios_len > 0 {
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
                let has_episodes = self.selected_episode_count() > 0;
                if has_episodes {
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
        let current = self.content.season_list_state.selected().unwrap_or(0);
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
            self.content.season_list_state.select(Some(next));
            self.content.selected_season_index = Some(next);
            self.content.selected_dubbing_index = None;
            self.content.dubbing_list_state.select(None);
            self.update_sidebar_for_season();
        }
    }

    fn move_selection_down(&mut self) {
        match self.focus {
            FocusPanel::SearchList => {
                let total = self.search.franchise_groups.len();
                if total == 0 {
                    return;
                }
                let i = match self.search.result_list_state.selected() {
                    Some(i) => {
                        if i >= total.saturating_sub(1) {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.search.result_list_state.select(Some(i));
                self.search.selected_group_index = Some(i);
                if let Some(group) = self.search.franchise_groups.get(i) {
                    self.search.selected_result_index = group.first().copied();
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
                    let i = match self.content.dubbing_list_state.selected() {
                        Some(i) => {
                            if i >= studios_len.saturating_sub(1) {
                                0
                            } else {
                                i + 1
                            }
                        }
                        None => 0,
                    };
                    self.content.dubbing_list_state.select(Some(i));
                    self.content.selected_dubbing_index = Some(i);
                }
            }
            FocusPanel::EpisodeList => {
                let episodes_len = self.selected_episode_count();
                if episodes_len == 0 {
                    return;
                }
                let i = match self.content.episode_list_state.selected() {
                    Some(i) => {
                        if i >= episodes_len.saturating_sub(1) {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.content.episode_list_state.select(Some(i));
                self.content.selected_episode_index = Some(i);
            }
        }
    }

    fn move_selection_up(&mut self) {
        match self.focus {
            FocusPanel::SearchList => {
                let total = self.search.franchise_groups.len();
                if total == 0 {
                    return;
                }
                let i = match self.search.result_list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            total.saturating_sub(1)
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.search.result_list_state.select(Some(i));
                self.search.selected_group_index = Some(i);
                if let Some(group) = self.search.franchise_groups.get(i) {
                    self.search.selected_result_index = group.first().copied();
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
                    let i = match self.content.dubbing_list_state.selected() {
                        Some(i) => {
                            if i == 0 {
                                studios_len.saturating_sub(1)
                            } else {
                                i - 1
                            }
                        }
                        None => 0,
                    };
                    self.content.dubbing_list_state.select(Some(i));
                    self.content.selected_dubbing_index = Some(i);
                }
            }
            FocusPanel::EpisodeList => {
                let episodes_len = self.selected_episode_count();
                if episodes_len == 0 {
                    return;
                }
                let i = match self.content.episode_list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            episodes_len.saturating_sub(1)
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.content.episode_list_state.select(Some(i));
                self.content.selected_episode_index = Some(i);
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

    fn reset_to_home(&mut self) {
        // Never auto-enter SearchInput: `/` is the only way to start typing.
        // Empty results show an empty-state hint instead of trapping the user.
        self.mode = AppMode::Normal;
        self.focus = FocusPanel::SearchList;
        self.search.query.clear();
        self.search.cursor = 0;
        self.content.current_sources = None;
        self.content.current_sources_key = None;
        self.content.current_details = None;
        self.content.studio_anime_ids.clear();
        self.content.sidebar_anime_idx = None;
        self.content.sidebar_subject_id = None;
        self.search
            .result_list_state
            .select(self.search.selected_group_index);
        self.search.selected_release_index = None;
        self.content.selected_season_index = None;
        self.content.season_list_state.select(None);
        self.content.selected_dubbing_index = None;
        self.content.dubbing_list_state.select(None);
        self.content.selected_episode_index = None;
        self.content.episode_list_state.select(None);
        self.loading = self.search.selected_result_index.is_some();
        self.activity_message = self
            .loading
            .then(|| "Завантаження вибраного аніме…".to_string());
        self.clear_status();
        self.current_poster = None;
        self.poster_fetch_pending = None;
        self.library.items.clear();
        self.library.all_items.clear();
        self.library.anime_index = None;
        self.library.anime_list_state.select(None);
        if self.mode == AppMode::Normal && self.search.selected_result_index.is_some() {
            self.restore_representative_poster();
        }
        self.search.ordering.popup = None;
        self.library.sort_popup = None;
        self.library.pending_watched_confirmation = None;
        self.library.pending_delete_confirmation = None;
        self.status_editor = None;
    }

    fn open_library_search(&mut self) {
        self.open_library();
        self.library.search_editing = true;
        self.library.search_cursor = self.library.search_query.chars().count();
        self.library.sort_popup = None;
        self.library.pending_watched_confirmation = None;
        self.library.pending_delete_confirmation = None;
        self.status_editor = None;
        self.clear_activity();
        self.clear_status();
    }

    pub fn open_library(&mut self) {
        if self.is_library_mode() {
            // Re-pressing 2 while already in the library jumps to the root list.
            if self.mode != AppMode::Library {
                self.mode = AppMode::Library;
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
                if let Some(index) = self.library.anime_index {
                    self.prepare_library_anime_selection();
                    self.library.anime_list_state.select(Some(index));
                }
            }
            return;
        }

        self.hydrate_library_catalog_metadata();
        self.library.all_items = build_library_items(&self.history);
        self.library.items.clear();
        self.mode = AppMode::Library;
        self.library.refresh_requested = true;
        self.apply_library_filter();
        if self.library.all_items.is_empty() {
            self.set_info_status("Бібліотека порожня — додайте аніме через e, або / для пошуку");
        }
    }

    pub fn library_selected_anime_id(&self) -> Option<u32> {
        if self.mode != AppMode::Library {
            if let Some(release) = self.library_selected_season() {
                return Some(release.anime_id);
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

    pub fn set_info_status(&mut self, message: impl Into<String>) {
        self.status_message = Some((message.into(), StatusKind::Info));
        self.status_expires_at = Some(Instant::now() + Duration::from_secs(4));
        self.status_retry_available = false;
    }

    pub fn set_error_status(&mut self, message: impl Into<String>) {
        self.loading = false;
        self.activity_message = None;
        self.status_message = Some((message.into(), StatusKind::Error));
        self.status_expires_at = None;
        self.status_retry_available = false;
    }

    pub fn set_retryable_error_status(&mut self, message: impl Into<String>) {
        self.set_error_status(message);
        self.status_retry_available = true;
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
        self.playback.now_playing = Some(NowPlaying {
            anime_id: target.anime_id,
            anime_title: target.anime_title.clone(),
            season: target.season,
            episode: target.episode,
            studio_name: target.studio_name.clone(),
            position: target.start_time.unwrap_or(0.0),
            duration: 0.0,
            paused: false,
        });
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
        self.status_expires_at = None;
        self.status_retry_available = false;
    }

    pub fn take_retry_request(&mut self) -> bool {
        std::mem::take(&mut self.retry_requested)
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
        self.playback.continue_request = match self.mode {
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
                .search
                .selected_group_index
                .and_then(|g_idx| {
                    if !self.content.studio_anime_ids.is_empty() {
                        let mut anime_ids = self.content.studio_anime_ids.clone();
                        anime_ids.sort_unstable();
                        anime_ids.dedup();
                        Some(ContinueRequest::Group {
                            anime_ids,
                            in_library: false,
                        })
                    } else {
                        self.search.franchise_groups.get(g_idx).map(|group| {
                            ContinueRequest::Group {
                                anime_ids: group
                                    .iter()
                                    .filter_map(|&idx| {
                                        self.search.results.get(idx).map(|anime| anime.id)
                                    })
                                    .collect(),
                                in_library: false,
                            }
                        })
                    }
                })
                .or(Some(ContinueRequest::Latest)),
        };
    }

    fn handle_pending_delete_confirmation(&mut self, key_code: KeyCode) -> bool {
        let Some((anime_ids, anime_title)) = self.library.pending_delete_confirmation.clone()
        else {
            return false;
        };

        match key_code {
            KeyCode::Enter => {
                self.library.pending_delete_confirmation = None;
                match self.storage.delete_library_entries(&anime_ids) {
                    Ok(history) => {
                        self.history = history;
                        self.rebuild_history_indexes();
                        self.reload_library_after_mutation();
                        self.set_info_status(format!("\"{}\" видалено з бібліотеки", anime_title));
                    }
                    Err(e) => self.set_error_status(format!("Не вдалося видалити аніме: {}", e)),
                }
                true
            }
            KeyCode::Esc => {
                self.library.pending_delete_confirmation = None;
                true
            }
            // Ignore other keys while the confirm dialog is open.
            _ => true,
        }
    }

    fn handle_clear_library_confirmation(&mut self, key_code: KeyCode) -> bool {
        if !self.library.clear_confirmation {
            return false;
        }
        match key_code {
            KeyCode::Enter => {
                self.library.clear_confirmation = false;
                match self.storage.clear_library() {
                    Ok(history) => {
                        self.history = history;
                        self.rebuild_history_indexes();
                        self.library.all_items.clear();
                        self.library.items.clear();
                        self.library.anime_index = None;
                        self.content.selected_season_index = None;
                        self.content.selected_dubbing_index = None;
                        self.content.selected_episode_index = None;
                        self.library.anime_list_state.select(None);
                        self.content.season_list_state.select(None);
                        self.content.dubbing_list_state.select(None);
                        self.content.episode_list_state.select(None);
                        self.content.current_sources = None;
                        self.content.current_sources_key = None;
                        self.content.current_details = None;
                        self.current_poster = None;
                        self.poster_fetch_pending = None;
                        self.set_info_status("Бібліотеку та прогрес очищено");
                    }
                    Err(error) => {
                        self.set_error_status(format!("Не вдалося очистити бібліотеку: {error}"));
                    }
                }
            }
            KeyCode::Esc => self.library.clear_confirmation = false,
            _ => {}
        }
        true
    }

    fn delete_library_selection(&mut self) {
        if self.mode == AppMode::Library {
            if let Some(anime) = self.library_selected_anime() {
                if !anime.anime_ids.is_empty() {
                    self.library.pending_delete_confirmation =
                        Some((anime.anime_ids.clone(), anime.anime_title.clone()));
                }
            }
        }
    }

    fn toggle_release_watched(
        &mut self,
        anime_id: u32,
        anime_title: String,
        release: LibraryReleaseMetadata,
    ) {
        let target_episodes = self.episode_targets_for_release(anime_id, &release);
        if target_episodes.is_empty() {
            self.set_info_status("Список серій цього випуску ще не завантажено");
            return;
        }

        let all_watched = self
            .history
            .library
            .get(&anime_id)
            .is_some_and(|record| record.status == AnimeStatus::Completed)
            || target_episodes.keys().all(|episode| {
                self.watched_index
                    .contains(&(anime_id, release.season, *episode))
            });
        let mark_watched = !all_watched;
        let episode_updates = target_episodes
            .into_iter()
            .map(|(episode, studio_name)| EpisodeWatchedUpdate {
                anime_id,
                anime_title: anime_title.clone(),
                season: release.season,
                episode,
                studio_name,
                watched: mark_watched,
            })
            .collect::<Vec<_>>();
        let status_update = AnimeStatusUpdate {
            anime_id,
            title: anime_title,
            status: if mark_watched && !release_metadata_is_ongoing(&release) {
                AnimeStatus::Completed
            } else {
                AnimeStatus::Watching
            },
            release: Some(release.clone()),
        };
        match self
            .storage
            .set_release_watched(&status_update, &episode_updates)
        {
            Ok(history) => {
                self.history = history;
                self.rebuild_history_indexes();
                if self.is_library_mode() {
                    self.reload_library_after_mutation();
                }
                self.set_info_status(if mark_watched {
                    format!("{} позначено як переглянутий", release.title)
                } else {
                    format!("{} позначено як непереглянутий", release.title)
                });
            }
            Err(error) => self.set_error_status(format!("Не вдалося оновити випуск: {error}")),
        }
    }

    fn toggle_selected_episode_watched(&mut self, anime_id: u32, anime_title: String, season: u32) {
        let Some(studio_name) = self
            .selected_dubbing_choice()
            .map(|choice| choice.studio_name().to_string())
        else {
            return;
        };
        let Some(episode) = self.selected_episode_choice() else {
            return;
        };
        let episode_number = episode.episode_number();
        let mark_watched = !self
            .watched_index
            .contains(&(anime_id, season, episode_number));
        match self.storage.set_episode_watched_across_dubbings(
            anime_id,
            &anime_title,
            season,
            episode_number,
            &studio_name,
            mark_watched,
        ) {
            Ok(history) => {
                self.history = history;
                self.rebuild_history_indexes();
                self.set_info_status(if mark_watched {
                    format!("Серію S{season}E{episode_number} позначено як переглянуту")
                } else {
                    format!("Серію S{season}E{episode_number} позначено як непереглянуту")
                });
            }
            Err(error) => {
                self.set_error_status(format!("Не вдалося оновити серію: {error}"));
            }
        }
    }

    fn toggle_library_selection_watched(&mut self) {
        match self.mode {
            AppMode::LibrarySeason | AppMode::LibraryDubbing => {
                let Some(anime) = self.library_selected_anime() else {
                    return;
                };
                let anime_title = anime.anime_title.clone();
                let Some(release) = self.library_selected_season().cloned() else {
                    return;
                };
                self.toggle_release_watched(release.anime_id, anime_title, release.metadata());
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
                self.toggle_selected_episode_watched(anime_id, anime_title, season_num);
            }
            _ => {}
        }
    }

    fn search_selected_season_anime_id(&self) -> Option<u32> {
        if self.has_release_catalog() {
            return self.selected_release_anihub_id();
        }
        let season_num = self.selected_season_num()?;
        self.content.current_sources.as_ref().and_then(|sources| {
            sources
                .ashdi
                .iter()
                .position(|studio| studio.season_number == season_num)
                .and_then(|idx| self.content.studio_anime_ids.get(idx))
                .copied()
        })
    }

    fn selected_episode_number(&self) -> Option<u32> {
        self.selected_episode_choice()
            .map(|episode| episode.episode_number())
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
            .selected_franchise_catalog()
            .map(|catalog| catalog.canonical_title.clone())
            .or_else(|| {
                self.search
                    .results
                    .iter()
                    .find(|anime| anime.id == anime_id)
                    .map(|anime| anime.title_ukrainian.clone())
            })
            .or_else(|| {
                self.content
                    .current_details
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
                let Some(catalog) = self.selected_franchise_catalog() else {
                    return;
                };
                let Some(release) = self.selected_release() else {
                    return;
                };
                let metadata = library_metadata_for_release(catalog, release);
                self.toggle_release_watched(anime_id, anime_title, metadata);
            }
            FocusPanel::EpisodeList => {
                self.toggle_selected_episode_watched(anime_id, anime_title, season_num);
            }
            FocusPanel::SearchList => {}
        }
    }

    fn reload_library_after_mutation(&mut self) {
        let prev_anime_title = self
            .library_selected_anime()
            .map(|anime| anime.anime_title.clone());
        let prev_mode = self.mode;
        let prev_release_id = (prev_mode != AppMode::Library)
            .then(|| self.library_selected_anime_id())
            .flatten();
        let prev_dubbing = self.content.selected_dubbing_index;
        let prev_episode = self.content.selected_episode_index;

        self.library.all_items = build_library_items(&self.history);
        self.apply_library_filter();

        self.library.anime_index = prev_anime_title
            .clone()
            .and_then(|anime_title| {
                self.library
                    .items
                    .iter()
                    .position(|item| item.anime_title == anime_title)
            })
            .or_else(|| (!self.library.items.is_empty()).then_some(0));
        self.library
            .anime_list_state
            .select(self.library.anime_index);

        if self.library.items.is_empty()
            || (self.mode != AppMode::Library && self.library_selected_anime().is_none())
        {
            self.mode = AppMode::Library;
        }

        let should_reprepare = match (&prev_anime_title, self.library_selected_anime()) {
            (Some(prev_title), Some(anime)) => anime.anime_title != *prev_title,
            (Some(_), None) => true,
            _ => self.content.current_sources.is_none(),
        };

        if should_reprepare && self.library_selected_anime().is_some() {
            self.prepare_library_anime_selection();
        }

        if let Some(anime_id) = prev_release_id {
            self.content.selected_season_index = self
                .library_selected_anime()
                .into_iter()
                .flat_map(|anime| anime.seasons.iter())
                .position(|release| release.anime_id == anime_id);
            self.content
                .season_list_state
                .select(self.content.selected_season_index);
        }
        if prev_mode == AppMode::LibraryDubbing || prev_mode == AppMode::LibraryEpisode {
            self.content.selected_dubbing_index = prev_dubbing.filter(|&idx| {
                self.selected_season_num()
                    .is_some_and(|sn| idx < self.dubbing_choices_for_season(sn).len())
            });
            self.content
                .dubbing_list_state
                .select(self.content.selected_dubbing_index);
        }
        if prev_mode == AppMode::LibraryEpisode {
            self.content.selected_episode_index =
                prev_episode.filter(|&idx| idx < self.selected_episode_count());
            self.content
                .episode_list_state
                .select(self.content.selected_episode_index);
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

        if self
            .content
            .current_details
            .as_ref()
            .map(|details| details.id)
            != Some(anime_id)
        {
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
        let j = self.content.current_sources.as_ref().and_then(|sources| {
            sources
                .ashdi
                .iter()
                .position(|s| s.season_number == season_num)
        });
        let j = match j {
            Some(j) => j,
            None => return,
        };
        let studio_owner_id = match self.content.studio_anime_ids.get(j).copied() {
            Some(id) => id,
            None => return,
        };
        let anime_id = studio_owner_id;
        if self.sidebar_subject() == Some(anime_id) {
            return;
        }
        self.select_sidebar_subject(Some(anime_id));
        if self.content.current_details.is_none() {
            self.loading = true;
            self.activity_message = Some("Завантаження метаданих випуску…".to_string());
        }
    }

    /// Відновлює постер першого TV-члена франшизи при поверненні до SearchList.
    fn restore_representative_poster(&mut self) {
        let representative_id = self
            .canonical_sidebar_subject()
            .or_else(|| self.content.studio_anime_ids.first().copied())
            .or_else(|| {
                self.search
                    .selected_result_index
                    .and_then(|i| self.search.results.get(i))
                    .map(|item| item.id)
            });

        self.select_sidebar_subject(representative_id);
    }
}

const fn library_filter_from_setting(filter: DefaultLibraryFilter) -> LibraryFilter {
    match filter {
        DefaultLibraryFilter::All => LibraryFilter::All,
        DefaultLibraryFilter::Watching => LibraryFilter::Watching,
        DefaultLibraryFilter::Planned => LibraryFilter::Planned,
        DefaultLibraryFilter::Completed => LibraryFilter::Completed,
        DefaultLibraryFilter::OnHold => LibraryFilter::OnHold,
        DefaultLibraryFilter::Dropped => LibraryFilter::Dropped,
    }
}

const fn library_filter_to_setting(filter: LibraryFilter) -> DefaultLibraryFilter {
    match filter {
        LibraryFilter::All => DefaultLibraryFilter::All,
        LibraryFilter::Watching => DefaultLibraryFilter::Watching,
        LibraryFilter::Planned => DefaultLibraryFilter::Planned,
        LibraryFilter::Completed => DefaultLibraryFilter::Completed,
        LibraryFilter::OnHold => DefaultLibraryFilter::OnHold,
        LibraryFilter::Dropped => DefaultLibraryFilter::Dropped,
    }
}

const fn library_sort_from_setting(sort: LibrarySortPreference) -> LibrarySort {
    match sort {
        LibrarySortPreference::Recent => LibrarySort::Recent,
        LibrarySortPreference::Title => LibrarySort::Title,
        LibrarySortPreference::Year => LibrarySort::Year,
        LibrarySortPreference::Rating => LibrarySort::Rating,
        LibrarySortPreference::Progress => LibrarySort::Progress,
    }
}

const fn library_sort_to_setting(sort: LibrarySort) -> LibrarySortPreference {
    match sort {
        LibrarySort::Recent => LibrarySortPreference::Recent,
        LibrarySort::Title => LibrarySortPreference::Title,
        LibrarySort::Year => LibrarySortPreference::Year,
        LibrarySort::Rating => LibrarySortPreference::Rating,
        LibrarySort::Progress => LibrarySortPreference::Progress,
    }
}

const fn search_sort_from_setting(sort: SearchSortPreference) -> SearchSort {
    match sort {
        SearchSortPreference::Relevance => SearchSort::Relevance,
        SearchSortPreference::Title => SearchSort::Title,
        SearchSortPreference::Year => SearchSort::Year,
        SearchSortPreference::Rating => SearchSort::Rating,
    }
}

const fn search_sort_to_setting(sort: SearchSort) -> SearchSortPreference {
    match sort {
        SearchSort::Relevance => SearchSortPreference::Relevance,
        SearchSort::Title => SearchSortPreference::Title,
        SearchSort::Year => SearchSortPreference::Year,
        SearchSort::Rating => SearchSortPreference::Rating,
    }
}

fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte_index, _)| byte_index)
}

fn compact_studio_name(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_studio_name(name: &str) -> String {
    let normalized = compact_studio_name(name);

    match normalized.as_str() {
        "fanwoxua" | "fanvoxua" => "fanvoxua".to_string(),
        "clankaizoku" | "кланкайзоку" => "clankaizoku".to_string(),
        "субтитри" | "subtitles" => "subtitles".to_string(),
        "субтитриclankaizoku" | "субтитрикланкайзоку" | "subtitlesclankaizoku" => {
            "subtitlesclankaizoku".to_string()
        }
        _ => normalized,
    }
}

pub fn canonical_studio_name(name: &str) -> &str {
    match compact_studio_name(name).as_str() {
        "fanwoxua" | "fanvoxua" => "FanVoxUA",
        "clankaizoku" | "кланкайзоку" => "Клан Кайзоку",
        "субтитриclankaizoku" | "субтитрикланкайзоку" => {
            "Субтитри · Клан Кайзоку"
        }
        "субтитри" | "subtitles" => "Субтитри",
        _ => name,
    }
}

fn richer_duplicate_label<'a>(
    sources: &'a EpisodeSourcesResponse,
    season_num: u32,
    normalized_name: &str,
) -> Option<&'a str> {
    if normalized_name != "subtitles" {
        return None;
    }
    sources
        .moonanime
        .iter()
        .filter(|studio| studio.season_number == season_num)
        .map(|studio| studio.studio_name.as_str())
        .find(|name| {
            let compact = compact_studio_name(name);
            (compact.starts_with("субтитри") || compact.starts_with("subtitles"))
                && normalize_studio_name(name) != normalized_name
        })
}

fn push_completed_episode(
    updates: &mut Vec<EpisodeWatchedUpdate>,
    seen: &mut HashSet<(u32, u32, u32)>,
    anime_id: u32,
    anime_title: &str,
    season: u32,
    episode: u32,
    studio_name: &str,
) {
    if seen.insert((anime_id, season, episode)) {
        updates.push(EpisodeWatchedUpdate {
            anime_id,
            anime_title: anime_title.to_string(),
            season,
            episode,
            studio_name: studio_name.to_string(),
            watched: true,
        });
    }
}

fn dubbing_choices_for_sources(
    sources: &EpisodeSourcesResponse,
    season_num: u32,
) -> Vec<DubbingChoice<'_>> {
    let mut seen_names = HashSet::new();
    let mut choices = Vec::new();
    for studio in sources
        .ashdi
        .iter()
        .filter(|studio| studio.season_number == season_num)
    {
        let source_name = normalize_studio_name(&studio.studio_name);
        let display_alias = richer_duplicate_label(sources, season_num, &source_name);
        let normalized_name = normalize_studio_name(display_alias.unwrap_or(&studio.studio_name));
        if seen_names.insert(normalized_name) {
            choices.push(DubbingChoice::Ashdi(studio, display_alias));
        }
    }
    for studio in sources
        .moonanime
        .iter()
        .filter(|studio| studio.season_number == season_num)
    {
        if seen_names.insert(normalize_studio_name(&studio.studio_name)) {
            choices.push(DubbingChoice::MoonAnime(studio));
        }
    }
    choices
}

fn sidebar_subject_for_release(release: &ReleaseEntry) -> Option<u32> {
    release
        .anihub_id
        .or_else(|| release.anilist_id.map(|id| ANILIST_POSTER_SUBJECT_BIT | id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_tabs_cycle_in_both_directions() {
        assert_eq!(SettingsTab::General.next(), SettingsTab::Themes);
        assert_eq!(SettingsTab::Themes.next(), SettingsTab::About);
        assert_eq!(SettingsTab::About.next(), SettingsTab::General);
        assert_eq!(SettingsTab::General.previous(), SettingsTab::About);
    }

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
            airing_status: None,
            next_airing_episode: None,
            next_airing_at: None,
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
    fn dubbings_are_unique_with_ashdi_before_additional_moonanime() {
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
                AshdiStudio {
                    id: 3,
                    studio_name: "amano gawa".to_string(),
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
    fn known_fanwox_typo_is_deduplicated_as_fanvox() {
        assert_eq!(canonical_studio_name("FanWoxUa"), "FanVoxUA");

        let sources = EpisodeSourcesResponse {
            ashdi: vec![AshdiStudio {
                id: 1,
                studio_name: "FanVoxUA".to_string(),
                season_number: 1,
                episodes: Vec::new(),
                episodes_count: 12,
            }],
            moonanime: vec![MoonAnimeSourceMarker {
                studio_name: "FanWoxUa".to_string(),
                season_number: 1,
                episodes_count: 12,
                episodes: Vec::new(),
            }],
        };

        let choices = dubbing_choices_for_sources(&sources, 1);
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].studio_name(), "FanVoxUA");
        assert!(!choices[0].is_moonanime());
    }

    #[test]
    fn transliterated_k_on_sources_are_deduplicated() {
        let sources = EpisodeSourcesResponse {
            ashdi: vec![
                AshdiStudio {
                    id: 1,
                    studio_name: "Клан Кайзоку".to_string(),
                    season_number: 2,
                    episodes: Vec::new(),
                    episodes_count: 10,
                },
                AshdiStudio {
                    id: 2,
                    studio_name: "Субтитри".to_string(),
                    season_number: 2,
                    episodes: Vec::new(),
                    episodes_count: 26,
                },
            ],
            moonanime: vec![
                MoonAnimeSourceMarker {
                    studio_name: "Clan Kaizoku".to_string(),
                    season_number: 2,
                    episodes_count: 16,
                    episodes: Vec::new(),
                },
                MoonAnimeSourceMarker {
                    studio_name: "Субтитри Clan Kaizoku".to_string(),
                    season_number: 2,
                    episodes_count: 16,
                    episodes: Vec::new(),
                },
            ],
        };

        let choices = dubbing_choices_for_sources(&sources, 2);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].studio_name(), "Клан Кайзоку");
        assert_eq!(choices[1].studio_name(), "Субтитри · Клан Кайзоку");
        assert!(choices.iter().all(|choice| !choice.is_moonanime()));
    }

    #[test]
    fn subtitle_credits_remain_distinct() {
        let sources = EpisodeSourcesResponse {
            ashdi: vec![AshdiStudio {
                id: 1,
                studio_name: "Субтитри".to_string(),
                season_number: 1,
                episodes: Vec::new(),
                episodes_count: 12,
            }],
            moonanime: vec![
                MoonAnimeSourceMarker {
                    studio_name: "Субтитри Team A".to_string(),
                    season_number: 1,
                    episodes_count: 12,
                    episodes: Vec::new(),
                },
                MoonAnimeSourceMarker {
                    studio_name: "Субтитри Team B".to_string(),
                    season_number: 1,
                    episodes_count: 12,
                    episodes: Vec::new(),
                },
            ],
        };

        let choices = dubbing_choices_for_sources(&sources, 1);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].studio_name(), "Субтитри Team A");
        assert_eq!(choices[1].studio_name(), "Субтитри Team B");
        assert!(!choices[0].is_moonanime());
        assert!(choices[1].is_moonanime());
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
        assert!(library_item_matches(&items[0], LibraryFilter::All, "тЕс"));
        assert!(!library_item_matches(
            &items[0],
            LibraryFilter::Completed,
            "тест"
        ));

        history.progress.values_mut().next().unwrap().watched = true;
        let items = build_library_items(&history);
        assert_eq!(items[0].status, AnimeStatus::Watching);
        assert!(!library_item_matches(
            &items[0],
            LibraryFilter::Completed,
            "тест"
        ));
    }

    #[test]
    fn library_sort_supports_titles_and_completion_ratio() {
        let mut history = AppHistory::default();
        for (anime_id, title, watched) in [(1, "Бета", 1), (2, "Альфа", 3)] {
            history.library.insert(
                anime_id,
                crate::storage::history::AnimeLibraryRecord {
                    title: title.to_string(),
                    status: AnimeStatus::Watching,
                    updated_at: i64::from(anime_id),
                    release: Some(LibraryReleaseMetadata {
                        title: title.to_string(),
                        kind: LibraryReleaseKind::Season,
                        season: 1,
                        part: Some(1),
                        episodes_count: Some(4),
                        first_episode: Some(1),
                        airing_status: None,
                        next_airing_episode: None,
                        next_airing_at: None,
                    }),
                },
            );
            for episode in 1..=watched {
                history.progress.insert(
                    StorageManager::make_progress_key(anime_id, 1, episode, "Dub"),
                    WatchProgress {
                        anime_id,
                        anime_title: title.to_string(),
                        season: 1,
                        episode,
                        studio_name: "Dub".to_string(),
                        timestamp: 1200.0,
                        duration: 1200.0,
                        watched: true,
                        updated_at: i64::from(anime_id),
                    },
                );
            }
        }
        let watched_index = history
            .progress
            .values()
            .filter(|progress| progress.watched)
            .map(|progress| (progress.anime_id, progress.season, progress.episode))
            .collect::<HashSet<_>>();
        let details_cache = moka::sync::Cache::new(4);
        let mut items = build_library_items(&history);

        sort_library_items(
            &mut items,
            LibrarySort::Title,
            false,
            &details_cache,
            &watched_index,
        );
        assert_eq!(items[0].anime_title, "Альфа");

        sort_library_items(
            &mut items,
            LibrarySort::Progress,
            false,
            &details_cache,
            &watched_index,
        );
        assert_eq!(items[0].anime_title, "Альфа");
        assert_eq!(
            library_progress_ratio(&items[0], &watched_index),
            Some((3, 4))
        );

        sort_library_items(
            &mut items,
            LibrarySort::Progress,
            true,
            &details_cache,
            &watched_index,
        );
        assert_eq!(items[0].anime_title, "Бета");
    }

    #[test]
    fn completion_updates_deduplicate_the_same_episode_across_dubbings() {
        let mut updates = Vec::new();
        let mut seen = HashSet::new();
        push_completed_episode(&mut updates, &mut seen, 7, "Тест", 1, 1, "Dub A");
        push_completed_episode(&mut updates, &mut seen, 7, "Тест", 1, 1, "Dub B");
        push_completed_episode(&mut updates, &mut seen, 7, "Тест", 1, 2, "Dub A");

        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|update| update.watched));
        assert_eq!(updates[0].studio_name, "Dub A");
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
                release: None,
            },
        );
        let items = build_library_items(&history);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].anime_title, "Каґуя");
        assert_eq!(items[0].status, AnimeStatus::Planned);

        history.library.get_mut(&42).unwrap().status = AnimeStatus::NotAdded;
        assert!(build_library_items(&history).is_empty());
    }

    #[test]
    fn library_materializes_unplayed_seasons_and_movies_from_status_metadata() {
        let mut history = AppHistory::default();
        for (anime_id, kind, season, title) in [
            (10, LibraryReleaseKind::Season, 1, "Перший сезон"),
            (20, LibraryReleaseKind::Season, 2, "Другий сезон"),
            (30, LibraryReleaseKind::Movie, 2, "Фільм після сезону"),
        ] {
            history.library.insert(
                anime_id,
                crate::storage::history::AnimeLibraryRecord {
                    title: "Франшиза".to_string(),
                    status: AnimeStatus::Completed,
                    updated_at: i64::from(anime_id),
                    release: Some(LibraryReleaseMetadata {
                        title: title.to_string(),
                        kind,
                        season,
                        part: Some(1),
                        episodes_count: Some(if kind == LibraryReleaseKind::Movie {
                            1
                        } else {
                            12
                        }),
                        first_episode: Some(1),
                        airing_status: None,
                        next_airing_episode: None,
                        next_airing_at: None,
                    }),
                },
            );
        }

        let items = build_library_items(&history);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].anime_ids, vec![10, 20, 30]);
        assert_eq!(items[0].seasons.len(), 3);
        assert_eq!(items[0].seasons[0].season, 1);
        assert_eq!(items[0].seasons[1].season, 2);
        assert_eq!(items[0].seasons[2].kind, LibraryReleaseKind::Movie);
        assert_eq!(items[0].status, AnimeStatus::Completed);
    }

    #[test]
    fn partially_completed_franchise_remains_in_watching_filter() {
        let mut history = AppHistory::default();
        for (anime_id, status) in [(10, AnimeStatus::Completed), (20, AnimeStatus::Planned)] {
            history.library.insert(
                anime_id,
                crate::storage::history::AnimeLibraryRecord {
                    title: "Франшиза".to_string(),
                    status,
                    updated_at: i64::from(anime_id),
                    release: None,
                },
            );
        }

        let items = build_library_items(&history);
        assert_eq!(items[0].status, AnimeStatus::Watching);
    }
}
