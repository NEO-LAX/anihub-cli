//! Search-result ordering and its small modal state.

use super::*;
use crate::api::build_franchise_catalogs;
use std::cmp::Ordering;

impl AppState {
    pub(crate) fn apply_search_results(
        &mut self,
        results: Vec<AnimeItem>,
        anilist_media: Vec<AniListMedia>,
        finish_search: bool,
    ) {
        self.search.results = results;
        self.search.anilist_media = anilist_media;
        for item in &self.search.results {
            self.details_cache.insert(item.id, AnimeDetails::from(item));
        }
        self.rebuild_search_projection();
        if finish_search {
            self.search.query.clear();
            self.search.cursor = 0;
        }

        self.focus = FocusPanel::SearchList;
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

        if !self.search.franchise_groups.is_empty() {
            self.search.result_list_state.select(Some(0));
            self.search.selected_group_index = Some(0);
            let representative = self.search.franchise_groups[0][0];
            self.search.selected_result_index = Some(representative);
            let canonical_id = self.search.results[representative].id;
            self.select_sidebar_subject(self.canonical_sidebar_subject().or(Some(canonical_id)));
            self.set_activity("Завантаження вибраного аніме…");
        } else {
            self.clear_activity();
            self.search.result_list_state.select(None);
            self.search.selected_group_index = None;
            self.search.selected_result_index = None;
            self.set_info_status("Нічого не знайдено");
        }
    }

    pub(crate) fn should_add_details_to_search(&self, details: &AnimeDetails) -> bool {
        details_belong_in_search(
            self.mode,
            self.search.results.iter().any(|item| item.id == details.id),
            details.anilist_id.is_some(),
        )
    }

    pub(crate) fn rebuild_search_projection(&mut self) {
        let selected_anchor = self
            .selected_franchise_catalog()
            .and_then(|catalog| catalog.anchor_anilist_id);
        let selected_title = self
            .selected_franchise_catalog()
            .map(|catalog| catalog.canonical_title.clone());
        let selected_release_anilist = self
            .selected_release()
            .and_then(|release| release.anilist_id);

        let catalogs = build_franchise_catalogs(&self.search.results, &self.search.anilist_media);
        let groups = catalogs
            .iter()
            .map(|catalog| {
                catalog
                    .releases
                    .iter()
                    .filter_map(|release| release.anihub_id)
                    .filter_map(|anime_id| {
                        self.search
                            .results
                            .iter()
                            .position(|item| item.id == anime_id)
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        self.search.franchise_catalogs = catalogs;
        self.search.franchise_groups = groups;
        self.sort_search_projection();

        if let Some(anchor) = selected_anchor {
            self.search.selected_group_index = self
                .search
                .franchise_catalogs
                .iter()
                .position(|catalog| catalog.anchor_anilist_id == Some(anchor));
        } else if let Some(title) = selected_title {
            self.search.selected_group_index = self
                .search
                .franchise_catalogs
                .iter()
                .position(|catalog| catalog.canonical_title == title);
        }
        if self.search.selected_group_index.is_none() && !self.search.franchise_catalogs.is_empty()
        {
            self.search.selected_group_index = Some(0);
        }
        if let Some(group_index) = self.search.selected_group_index {
            self.search.selected_result_index = self
                .search
                .franchise_groups
                .get(group_index)
                .and_then(|group| group.first())
                .copied();
            if let Some(anilist_id) = selected_release_anilist {
                self.search.selected_release_index = self.search.franchise_catalogs[group_index]
                    .releases
                    .iter()
                    .position(|release| release.anilist_id == Some(anilist_id));
                self.content
                    .season_list_state
                    .select(self.search.selected_release_index);
            }
        }
        if self.focus != FocusPanel::SearchList {
            self.refresh_selected_release();
        }
    }

    pub(super) fn open_search_sort_popup(&mut self) {
        if self.focus != FocusPanel::SearchList || self.search.franchise_groups.is_empty() {
            return;
        }
        let selected = SearchSort::ALL
            .iter()
            .position(|sort| *sort == self.search.ordering.sort)
            .unwrap_or(0);
        self.search.ordering.popup = Some(selected);
    }

    pub(super) fn handle_search_sort_popup(&mut self, key_code: KeyCode) -> bool {
        let Some(selected) = self.search.ordering.popup else {
            return false;
        };
        match key_code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.search.ordering.popup = Some(selected.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.search.ordering.popup = Some((selected + 1).min(SearchSort::ALL.len() - 1));
            }
            KeyCode::Enter => {
                let selected_sort = SearchSort::ALL[selected];
                if selected_sort == self.search.ordering.sort {
                    self.search.ordering.reversed = !self.search.ordering.reversed;
                } else {
                    self.search.ordering.sort = selected_sort;
                    self.search.ordering.reversed = false;
                }
                self.settings.search_sort = search_sort_to_setting(self.search.ordering.sort);
                self.settings.search_sort_reversed = self.search.ordering.reversed;
                self.search.ordering.popup = None;
                self.sort_search_projection();
            }
            KeyCode::Esc | KeyCode::Char('s') => self.search.ordering.popup = None,
            _ => {}
        }
        true
    }

    /// Reorder aligned franchise catalogs/groups without touching raw API
    /// results. Keeping the raw order makes the Relevance option reversible.
    pub fn sort_search_projection(&mut self) {
        let selected_identity = self.search.selected_group_index.and_then(|index| {
            self.search
                .franchise_catalogs
                .get(index)
                .map(|catalog| (catalog.anchor_anilist_id, catalog.canonical_title.clone()))
        });
        let mut projection = std::mem::take(&mut self.search.franchise_catalogs)
            .into_iter()
            .zip(std::mem::take(&mut self.search.franchise_groups))
            .collect::<Vec<_>>();
        let sort = self.search.ordering.sort;
        let reversed = self.search.ordering.reversed;
        let results = &self.search.results;
        projection.sort_by(|(a_catalog, a_group), (b_catalog, b_group)| {
            let ordering =
                compare_search_entries(sort, a_catalog, a_group, b_catalog, b_group, results);
            if reversed {
                ordering.reverse()
            } else {
                ordering
            }
        });
        (self.search.franchise_catalogs, self.search.franchise_groups) =
            projection.into_iter().unzip();

        self.search.selected_group_index = selected_identity
            .and_then(|(anchor, title)| {
                self.search.franchise_catalogs.iter().position(|catalog| {
                    (anchor.is_some() && catalog.anchor_anilist_id == anchor)
                        || catalog.canonical_title == title
                })
            })
            .or_else(|| (!self.search.franchise_groups.is_empty()).then_some(0));
        self.search
            .result_list_state
            .select(self.search.selected_group_index);
        self.search.selected_result_index = self
            .search
            .selected_group_index
            .and_then(|index| self.search.franchise_groups.get(index))
            .and_then(|group| group.first())
            .copied();
    }
}

fn details_belong_in_search(mode: AppMode, already_present: bool, has_anilist_id: bool) -> bool {
    mode == AppMode::Normal && !already_present && has_anilist_id
}

fn compare_search_entries(
    sort: SearchSort,
    a_catalog: &FranchiseCatalog,
    a_group: &[usize],
    b_catalog: &FranchiseCatalog,
    b_group: &[usize],
    results: &[AnimeItem],
) -> Ordering {
    match sort {
        SearchSort::Relevance => relevance_rank(a_group).cmp(&relevance_rank(b_group)),
        SearchSort::Title => a_catalog
            .canonical_title
            .to_lowercase()
            .cmp(&b_catalog.canonical_title.to_lowercase()),
        SearchSort::Year => compare_optional_desc(
            search_year(a_catalog, a_group, results),
            search_year(b_catalog, b_group, results),
            Ord::cmp,
        ),
        SearchSort::Rating => compare_optional_desc(
            search_rating(a_catalog, a_group, results),
            search_rating(b_catalog, b_group, results),
            f32::total_cmp,
        ),
    }
    .then_with(|| {
        a_catalog
            .canonical_title
            .to_lowercase()
            .cmp(&b_catalog.canonical_title.to_lowercase())
    })
}

fn relevance_rank(group: &[usize]) -> usize {
    group.iter().copied().min().unwrap_or(usize::MAX)
}

fn search_year(catalog: &FranchiseCatalog, group: &[usize], results: &[AnimeItem]) -> Option<u32> {
    catalog
        .releases
        .iter()
        .filter(|release| release.classification == ReleaseClassification::MainlineSeason)
        .filter_map(|release| release.year)
        .min()
        .or_else(|| {
            group
                .iter()
                .filter_map(|index| results.get(*index)?.year)
                .min()
        })
}

fn search_rating(
    catalog: &FranchiseCatalog,
    group: &[usize],
    results: &[AnimeItem],
) -> Option<f32> {
    catalog
        .releases
        .iter()
        .filter(|release| release.classification == ReleaseClassification::MainlineSeason)
        .filter_map(|release| release.rating)
        .max_by(f32::total_cmp)
        .or_else(|| {
            group
                .iter()
                .filter_map(|index| results.get(*index)?.rating)
                .max_by(f32::total_cmp)
        })
}

fn compare_optional_desc<T>(
    a: Option<T>,
    b: Option<T>,
    compare: impl Fn(&T, &T) -> Ordering,
) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => compare(&b, &a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(title: &str, year: u32, rating: f32) -> FranchiseCatalog {
        FranchiseCatalog {
            anchor_anilist_id: None,
            canonical_title: title.to_string(),
            canonical_poster_url: None,
            unresolved_anilist_ids: Vec::new(),
            releases: vec![ReleaseEntry {
                anihub_id: None,
                anilist_id: None,
                title: title.to_string(),
                anime_type: "TV".to_string(),
                year: Some(year),
                poster_url: None,
                episodes_count: None,
                available_episodes: None,
                airing_status: None,
                next_airing_episode: None,
                next_airing_at: None,
                description: None,
                rating: Some(rating),
                genres: None,
                dubbing_studios: None,
                conceptual_season: Some(1),
                part: Some(1),
                classification: ReleaseClassification::MainlineSeason,
                availability: ReleaseAvailability::Unavailable,
            }],
        }
    }

    #[test]
    fn search_sort_orders_franchises_by_each_selected_key() {
        let alpha = catalog("Альфа", 2020, 7.0);
        let beta = catalog("Бета", 2024, 9.0);
        let no_results = Vec::new();

        assert_eq!(
            compare_search_entries(
                SearchSort::Relevance,
                &alpha,
                &[5],
                &beta,
                &[1],
                &no_results,
            ),
            Ordering::Greater
        );
        assert_eq!(
            compare_search_entries(SearchSort::Title, &alpha, &[5], &beta, &[1], &no_results,),
            Ordering::Less
        );
        assert_eq!(
            compare_search_entries(SearchSort::Year, &alpha, &[5], &beta, &[1], &no_results,),
            Ordering::Greater
        );
        assert_eq!(
            compare_search_entries(SearchSort::Rating, &alpha, &[5], &beta, &[1], &no_results,),
            Ordering::Greater
        );
    }

    #[test]
    fn details_merge_requires_search_mode_new_id_and_anilist_identity() {
        assert!(details_belong_in_search(AppMode::Normal, false, true));
        assert!(!details_belong_in_search(AppMode::Library, false, true));
        assert!(!details_belong_in_search(AppMode::Normal, true, true));
        assert!(!details_belong_in_search(AppMode::Normal, false, false));
    }
}
