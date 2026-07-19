//! Library-owned models and transient UI state.

use crate::storage::{AnimeStatus, LibraryReleaseKind, LibraryReleaseMetadata, WatchProgress};
use ratatui::widgets::ListState;

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

    pub(super) const fn next(self) -> Self {
        match self {
            Self::All => Self::Watching,
            Self::Watching => Self::Planned,
            Self::Planned => Self::Completed,
            Self::Completed => Self::OnHold,
            Self::OnHold => Self::Dropped,
            Self::Dropped => Self::All,
        }
    }

    pub(super) const fn previous(self) -> Self {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibrarySort {
    Recent,
    Title,
    Year,
    Rating,
    Progress,
}

impl LibrarySort {
    pub const ALL: [Self; 5] = [
        Self::Recent,
        Self::Title,
        Self::Year,
        Self::Rating,
        Self::Progress,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Recent => "Нещодавні",
            Self::Title => "Назва",
            Self::Year => "Рік",
            Self::Rating => "Рейтинг",
            Self::Progress => "Прогрес",
        }
    }

    pub const fn order_label(self, reversed: bool) -> &'static str {
        match (self, reversed) {
            (Self::Recent | Self::Year, false) => "новіші → старіші",
            (Self::Recent | Self::Year, true) => "старіші → новіші",
            (Self::Title, false) => "А → Я",
            (Self::Title, true) => "Я → А",
            (Self::Rating, false) => "вищий → нижчий",
            (Self::Rating, true) => "нижчий → вищий",
            (Self::Progress, false) => "більший → менший",
            (Self::Progress, true) => "менший → більший",
        }
    }

    pub const fn direction_symbol(self, reversed: bool) -> &'static str {
        let ascending = matches!(self, Self::Title) != reversed;
        if ascending { "↑" } else { "↓" }
    }
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
    pub part: Option<u32>,
    pub title: String,
    pub kind: LibraryReleaseKind,
    pub episodes_count: Option<u32>,
    pub first_episode: Option<u32>,
    pub airing_status: Option<String>,
    pub next_airing_episode: Option<u32>,
    pub next_airing_at: Option<i64>,
    pub status: AnimeStatus,
    pub episodes: Vec<WatchProgress>,
}

impl LibrarySeasonEntry {
    pub(super) fn metadata(&self) -> LibraryReleaseMetadata {
        LibraryReleaseMetadata {
            title: self.title.clone(),
            kind: self.kind,
            season: self.season,
            part: self.part,
            episodes_count: self.episodes_count,
            first_episode: self.first_episode,
            airing_status: self.airing_status.clone(),
            next_airing_episode: self.next_airing_episode,
            next_airing_at: self.next_airing_at,
        }
    }
}

#[derive(Clone)]
pub struct LibraryWatchedConfirmation {
    pub anime_title: String,
    pub releases: Vec<LibrarySeasonEntry>,
    pub mark_watched: bool,
}

pub struct LibraryState {
    pub items: Vec<LibraryAnimeEntry>,
    pub all_items: Vec<LibraryAnimeEntry>,
    pub filter: LibraryFilter,
    pub sort: LibrarySort,
    pub sort_reversed: bool,
    pub sort_popup: Option<usize>,
    pub search_query: String,
    pub search_cursor: usize,
    pub search_editing: bool,
    pub anime_index: Option<usize>,
    pub anime_list_state: ListState,
    pub pending_delete_confirmation: Option<(Vec<u32>, String)>,
    pub pending_watched_confirmation: Option<LibraryWatchedConfirmation>,
    pub clear_confirmation: bool,
    pub refresh_requested: bool,
}

impl LibraryState {
    pub(super) fn new(filter: LibraryFilter, sort: LibrarySort, sort_reversed: bool) -> Self {
        Self {
            items: Vec::new(),
            all_items: Vec::new(),
            filter,
            sort,
            sort_reversed,
            sort_popup: None,
            search_query: String::new(),
            search_cursor: 0,
            search_editing: false,
            anime_index: None,
            anime_list_state: ListState::default(),
            pending_delete_confirmation: None,
            pending_watched_confirmation: None,
            clear_confirmation: false,
            refresh_requested: false,
        }
    }
}
