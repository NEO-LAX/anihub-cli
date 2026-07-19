//! Search-result ordering and its small modal state.

use super::*;
use std::cmp::Ordering;

impl AppState {
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
}
