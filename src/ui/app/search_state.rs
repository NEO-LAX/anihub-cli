//! State owned by the Search tab.
//!
//! Keeping query editing and result selection together makes these invariants
//! testable without constructing the application-wide state.

use crate::api::{AniListMedia, AnimeItem, FranchiseCatalog};
use ratatui::widgets::ListState;

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
    pub(super) fn new(sort: SearchSort, reversed: bool) -> Self {
        Self {
            sort,
            reversed,
            popup: None,
        }
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
    pub(super) fn new(
        ordering: SearchOrderingState,
        franchise_catalogs: Vec<FranchiseCatalog>,
    ) -> Self {
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

    pub(super) fn insert_char(&mut self, character: char) {
        let byte_index = byte_index_for_char(&self.query, self.cursor);
        self.query.insert(byte_index, character);
        self.cursor += 1;
    }

    pub(super) fn backspace_char(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = byte_index_for_char(&self.query, self.cursor - 1);
        let end = byte_index_for_char(&self.query, self.cursor);
        self.query.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(super) fn delete_char(&mut self) {
        let char_count = self.query.chars().count();
        if self.cursor >= char_count {
            return;
        }
        let start = byte_index_for_char(&self.query, self.cursor);
        let end = byte_index_for_char(&self.query, self.cursor + 1);
        self.query.replace_range(start..end, "");
    }
}

fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte_index, _)| byte_index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(query: &str, cursor: usize) -> SearchState {
        let mut state = SearchState::new(SearchOrderingState::default(), Vec::new());
        state.query = query.to_string();
        state.cursor = cursor;
        state
    }

    #[test]
    fn unicode_query_editing_keeps_cursor_on_character_boundaries() {
        let mut search = state("Наруто", 1);

        search.insert_char('е');
        assert_eq!(search.query, "Неаруто");
        assert_eq!(search.cursor, 2);

        search.backspace_char();
        assert_eq!(search.query, "Наруто");
        assert_eq!(search.cursor, 1);

        search.delete_char();
        assert_eq!(search.query, "Нруто");
        assert_eq!(search.cursor, 1);
    }

    #[test]
    fn query_editing_is_safe_at_both_edges() {
        let mut search = state("Каґуя", 0);
        search.backspace_char();
        assert_eq!(search.query, "Каґуя");

        search.cursor = search.query.chars().count();
        search.delete_char();
        assert_eq!(search.query, "Каґуя");
    }
}
